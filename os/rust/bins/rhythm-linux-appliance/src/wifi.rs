//! Linux appliance Wi-Fi helpers for rpiz provisioning and recovery.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use rhythm_os::provisioning::WifiCredentials;
use serde::Serialize;

const WPA_CONF: &str = "/etc/wpa_supplicant.conf";
const WIFI_INIT_SCRIPT: &str = "/etc/init.d/S42wifi";
const WPA_CLI: &str = "/usr/sbin/wpa_cli";
const DEFAULT_COUNTRY: &str = "US";

/// Bound on the Wi-Fi init-script run. `Command::status()` has no timeout,
/// and a wedged wpa_supplicant/dhcp stop can hang the init script — during
/// startup that happens BEFORE the event loop and liveness watchdog exist,
/// so an unbounded wait bricks the appliance until a manual power cycle.
const WIFI_SCRIPT_TIMEOUT: Duration = Duration::from_secs(60);
/// Bound on a `wpa_cli status` query (runs on every Wi-Fi status poll).
const WPA_CLI_TIMEOUT: Duration = Duration::from_secs(5);
/// Short settle window for scan results after `wpa_cli scan` returns `OK`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const WPA_SCAN_SETTLE_DELAY: Duration = Duration::from_secs(2);

/// Wait for a spawned child with a deadline, killing it on timeout.
pub(crate) fn wait_child_with_timeout(
    mut child: std::process::Child,
    timeout: Duration,
    label: &str,
) -> Result<std::process::Output> {
    let deadline = Instant::now() + timeout;
    loop {
        match child
            .try_wait()
            .with_context(|| format!("waiting for {label}"))?
        {
            Some(_) => {
                return child
                    .wait_with_output()
                    .with_context(|| format!("collecting output of {label}"));
            }
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    bail!("{label} timed out after {timeout:?}");
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct WifiStatus {
    pub interface: Option<String>,
    pub config_present: bool,
    pub connected: bool,
    pub ssid: Option<String>,
    pub ip_address: Option<String>,
    pub wpa_state: Option<String>,
}

pub fn status_snapshot() -> WifiStatus {
    let interface = find_wifi_iface();
    let config_present = has_wifi_config();
    let ssid = interface
        .as_deref()
        .and_then(|iface| wpa_status_field(iface, "ssid"));
    let ip_address = interface
        .as_deref()
        .and_then(|iface| wpa_status_field(iface, "ip_address"));
    let wpa_state = interface
        .as_deref()
        .and_then(|iface| wpa_status_field(iface, "wpa_state"));
    let connected = ip_address.is_some() || matches!(wpa_state.as_deref(), Some("COMPLETED"));

    WifiStatus {
        interface,
        config_present,
        connected,
        ssid,
        ip_address,
        wpa_state,
    }
}

pub fn has_active_connection() -> bool {
    status_snapshot().ip_address.is_some()
}

pub fn has_wifi_config() -> bool {
    fs::read_to_string(WPA_CONF)
        .map(|content| {
            content
                .lines()
                .any(|line| line.trim_start().starts_with("network={"))
        })
        .unwrap_or(false)
}

pub fn load_configured_credentials() -> Result<Option<WifiCredentials>> {
    let content = match fs::read_to_string(WPA_CONF) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading {}", WPA_CONF)),
    };
    let preferred_ssid = find_wifi_iface()
        .as_deref()
        .and_then(|iface| wpa_status_field(iface, "ssid"));
    Ok(parse_wpa_credentials(&content, preferred_ssid.as_deref()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WifiScanNetwork {
    pub ssid: String,
    pub rssi: i32,
    pub security: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency: Option<u32>,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn scan_networks() -> Result<Vec<WifiScanNetwork>> {
    let iface = find_wifi_iface().ok_or_else(|| anyhow::anyhow!("No Wi-Fi interface found"))?;
    let scan_error = run_wpa_cli(&iface, &["scan"], "wpa_cli scan").err();

    std::thread::sleep(WPA_SCAN_SETTLE_DELAY);

    let scan_results = match run_wpa_cli(&iface, &["scan_results"], "wpa_cli scan_results") {
        Ok(output) => output,
        Err(results_error) => {
            if let Some(scan_error) = scan_error {
                return Err(scan_error).with_context(|| {
                    format!("Wi-Fi scan results were unavailable: {results_error}")
                });
            }
            return Err(results_error);
        }
    };

    let networks = parse_wpa_scan_results(&scan_results);
    if networks.is_empty() {
        if let Some(scan_error) = scan_error {
            return Err(scan_error
                .context("Wi-Fi scan request failed and no cached scan results were available"));
        }
    }
    Ok(networks)
}

pub fn connect_with_credentials(creds: &WifiCredentials, timeout: Duration) -> Result<String> {
    write_wifi_credentials(creds)?;
    restart_wifi()?;
    wait_for_ip(timeout)
}

pub fn connect_with_credentials_or_restore(
    creds: &WifiCredentials,
    timeout: Duration,
) -> Result<String> {
    let previous_credentials = load_configured_credentials()?;

    match connect_with_credentials(creds, timeout) {
        Ok(ip) => Ok(ip),
        Err(error) => {
            match previous_credentials {
                Some(previous) => {
                    let _ = connect_with_credentials(&previous, timeout);
                }
                None => {
                    let _ = clear_credentials_and_restart();
                }
            }
            Err(error)
        }
    }
}

pub fn clear_credentials_and_restart() -> Result<()> {
    clear_wifi_credentials()?;
    restart_wifi()
}

pub fn clear_credentials() -> Result<()> {
    clear_wifi_credentials()
}

/// Maximum SSID length per 802.11 (bytes, not chars).
const MAX_SSID_BYTES: usize = 32;

/// Reject credentials that cannot be represented safely in a quoted
/// `wpa_supplicant.conf` value. A newline (or any other control character)
/// inside `ssid="..."`/`psk="..."` breaks out of the quoted value and
/// invalidates the entire config file, taking Wi-Fi down until the appliance
/// is re-provisioned over BLE.
pub fn validate_credentials(creds: &WifiCredentials) -> Result<()> {
    if creds.ssid.trim().is_empty() {
        bail!("SSID must not be empty");
    }
    if creds.ssid.len() > MAX_SSID_BYTES {
        bail!("SSID exceeds {} bytes", MAX_SSID_BYTES);
    }
    if creds.ssid.chars().any(char::is_control) {
        bail!("SSID contains control characters");
    }
    if creds.password.chars().any(char::is_control) {
        bail!("Wi-Fi password contains control characters");
    }
    Ok(())
}

fn write_wifi_credentials(creds: &WifiCredentials) -> Result<()> {
    validate_credentials(creds)?;
    let country = existing_country().unwrap_or_else(|| DEFAULT_COUNTRY.to_string());
    let body = render_wpa_conf(creds, &country);
    write_wpa_conf(&body)
}

/// Render a `wpa_supplicant.conf` body for the given credentials + country.
/// Extracted so it can be tested without touching the filesystem.
fn render_wpa_conf(creds: &WifiCredentials, country: &str) -> String {
    let mut body = String::new();
    body.push_str("ctrl_interface=/var/run/wpa_supplicant\n");
    body.push_str("update_config=0\n");
    body.push_str(&format!("country={}\n\n", country));
    body.push_str("network={\n");
    body.push_str(&format!("  ssid=\"{}\"\n", escape_wpa_value(&creds.ssid)));
    body.push_str(&format!(
        "  psk=\"{}\"\n",
        escape_wpa_value(&creds.password)
    ));
    body.push_str("}\n");
    body
}

fn clear_wifi_credentials() -> Result<()> {
    let country = existing_country().unwrap_or_else(|| DEFAULT_COUNTRY.to_string());
    let body = format!(
        "ctrl_interface=/var/run/wpa_supplicant\nupdate_config=0\ncountry={}\n\n# Wi-Fi credentials cleared.\n",
        country
    );
    write_wpa_conf(&body)
}

fn write_wpa_conf(body: &str) -> Result<()> {
    write_atomic_0600(Path::new(WPA_CONF), body)
}

/// Durably replace `path` with `body`: write a mode-0600 temp sibling, fsync
/// it, atomically rename into place, then fsync the parent directory. A power
/// loss mid-write can no longer truncate the only network config the
/// appliance has.
fn write_atomic_0600(path: &Path, body: &str) -> Result<()> {
    use std::io::Write;

    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    fs::create_dir_all(&parent).with_context(|| format!("creating {}", parent.display()))?;

    let mut tmp_name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    tmp_name.push(".tmp");
    let tmp = parent.join(tmp_name);
    if tmp.exists() {
        let _ = fs::remove_file(&tmp);
    }

    {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        file.write_all(body.as_bytes())
            .with_context(|| format!("writing {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("syncing {}", tmp.display()))?;
    }

    if let Err(error) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(error)
            .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()));
    }

    if let Ok(dir) = fs::File::open(&parent) {
        let _ = dir.sync_all();
    }

    Ok(())
}

fn existing_country() -> Option<String> {
    fs::read_to_string(WPA_CONF).ok().and_then(|content| {
        content.lines().find_map(|line| {
            line.strip_prefix("country=")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.to_ascii_uppercase())
        })
    })
}

fn restart_wifi() -> Result<()> {
    run_wifi_script("restart")
}

fn run_wifi_script(action: &str) -> Result<()> {
    let child = Command::new(WIFI_INIT_SCRIPT)
        .arg(action)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("running {} {}", WIFI_INIT_SCRIPT, action))?;
    let output = wait_child_with_timeout(
        child,
        WIFI_SCRIPT_TIMEOUT,
        &format!("{} {}", WIFI_INIT_SCRIPT, action),
    )?;
    if !output.status.success() {
        bail!(
            "{} {} failed with status {}",
            WIFI_INIT_SCRIPT,
            action,
            output.status
        );
    }
    Ok(())
}

fn wait_for_ip(timeout: Duration) -> Result<String> {
    let deadline = Instant::now() + timeout;
    let mut last = status_snapshot();

    while Instant::now() < deadline {
        last = status_snapshot();
        if let Some(ip) = last.ip_address.clone() {
            return Ok(ip);
        }
        std::thread::sleep(Duration::from_secs(1));
    }

    let state = last.wpa_state.unwrap_or_else(|| "unknown".to_string());
    let iface = last.interface.unwrap_or_else(|| "wlan?".to_string());
    if let Some(ssid) = last.ssid {
        bail!(
            "Wi-Fi did not get an IP on {} (state={}, ssid='{}')",
            iface,
            state,
            ssid
        );
    }
    bail!("Wi-Fi did not get an IP on {} (state={})", iface, state)
}

fn find_wifi_iface() -> Option<String> {
    let netdir = Path::new("/sys/class/net");
    let entries = fs::read_dir(netdir).ok()?;

    for entry in entries.flatten() {
        let path = entry.path();
        let iface = entry.file_name().to_string_lossy().into_owned();
        if iface == "lo" {
            continue;
        }
        if iface.starts_with("wlan") || iface.starts_with("wl") {
            return Some(iface);
        }
        if path.join("wireless").is_dir() || path.join("phy80211").exists() {
            return Some(iface);
        }
    }

    None
}

fn wpa_status_field(iface: &str, field: &str) -> Option<String> {
    let stdout = run_wpa_cli(iface, &["status"], "wpa_cli status").ok()?;
    stdout.lines().find_map(|line| {
        line.strip_prefix(&format!("{field}="))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn run_wpa_cli(iface: &str, args: &[&str], label: &str) -> Result<String> {
    let child = Command::new(WPA_CLI)
        .args(["-i", iface])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("running {label}"))?;
    let output = wait_child_with_timeout(child, WPA_CLI_TIMEOUT, label)?;
    if !output.status.success() {
        bail!("{label} failed with status {}", output.status);
    }
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if stdout.lines().any(|line| line.trim() == "FAIL") {
        bail!("{label} returned FAIL");
    }
    Ok(stdout)
}

fn parse_wpa_scan_results(output: &str) -> Vec<WifiScanNetwork> {
    let mut networks: Vec<WifiScanNetwork> = Vec::new();

    for line in output.lines().skip(1) {
        let mut parts = line.splitn(5, '\t');
        let _bssid = parts.next();
        let frequency = parts.next().and_then(|value| value.parse::<u32>().ok());
        let Some(rssi) = parts.next().and_then(|value| value.parse::<i32>().ok()) else {
            continue;
        };
        let flags = parts.next().unwrap_or_default();
        let ssid = parts.next().unwrap_or_default();
        if ssid.trim().is_empty() || ssid.chars().any(char::is_control) {
            continue;
        }

        let candidate = WifiScanNetwork {
            ssid: ssid.to_string(),
            rssi,
            security: scan_security_label(flags).to_string(),
            frequency,
        };

        match networks
            .iter_mut()
            .find(|network| network.ssid == candidate.ssid)
        {
            Some(existing) if candidate.rssi > existing.rssi => *existing = candidate,
            Some(_) => {}
            None => networks.push(candidate),
        }
    }

    networks.sort_by(|left, right| {
        right
            .rssi
            .cmp(&left.rssi)
            .then_with(|| left.ssid.to_lowercase().cmp(&right.ssid.to_lowercase()))
    });
    networks
}

fn scan_security_label(flags: &str) -> &'static str {
    let normalized = flags.to_ascii_uppercase();
    if normalized.contains("SAE") || normalized.contains("WPA3") {
        "wpa3"
    } else if normalized.contains("WPA2") || normalized.contains("RSN") {
        "wpa2"
    } else if normalized.contains("WPA") {
        "wpa"
    } else if normalized.contains("WEP") {
        "wep"
    } else {
        "open"
    }
}

fn escape_wpa_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedWifiNetwork {
    ssid: String,
    password: String,
}

fn parse_wpa_credentials(content: &str, preferred_ssid: Option<&str>) -> Option<WifiCredentials> {
    let mut networks = Vec::new();
    let mut in_network = false;
    let mut ssid: Option<String> = None;
    let mut password: Option<String> = None;

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if !in_network {
            if line == "network={" {
                in_network = true;
                ssid = None;
                password = None;
            }
            continue;
        }

        if line == "}" {
            if let (Some(ssid), Some(password)) = (ssid.take(), password.take()) {
                networks.push(ParsedWifiNetwork { ssid, password });
            }
            in_network = false;
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "ssid" => ssid = parse_wpa_scalar(value.trim()),
            "psk" => password = parse_wpa_scalar(value.trim()),
            _ => {}
        }
    }

    let selected = preferred_ssid
        .and_then(|preferred| networks.iter().find(|network| network.ssid == preferred))
        .or(match networks.as_slice() {
            [single] => Some(single),
            _ => None,
        })?;

    Some(WifiCredentials {
        ssid: selected.ssid.clone(),
        password: selected.password.clone(),
    })
}

fn parse_wpa_scalar(value: &str) -> Option<String> {
    if value.starts_with('"') {
        parse_wpa_quoted_scalar(value)
    } else {
        value
            .split('#')
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    }
}

fn parse_wpa_quoted_scalar(value: &str) -> Option<String> {
    let mut chars = value.chars();
    if chars.next()? != '"' {
        return None;
    }

    let mut parsed = String::new();
    let mut escaped = false;
    for ch in chars {
        if escaped {
            parsed.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return Some(parsed),
            _ => parsed.push(ch),
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_wpa_value_doubles_backslashes_and_escapes_quotes() {
        assert_eq!(escape_wpa_value("plain"), "plain");
        assert_eq!(escape_wpa_value("has\"quote"), "has\\\"quote");
        assert_eq!(escape_wpa_value("back\\slash"), "back\\\\slash");
        assert_eq!(
            escape_wpa_value("mix\\of\"both"),
            "mix\\\\of\\\"both",
            "both quote and backslash must be escaped in a single value"
        );
    }

    #[test]
    fn escape_wpa_value_preserves_spaces_and_unicode() {
        assert_eq!(
            escape_wpa_value("My Wi-Fi"),
            "My Wi-Fi",
            "spaces are legal inside the quoted wpa value"
        );
        assert_eq!(escape_wpa_value("café 2.4GHz"), "café 2.4GHz");
    }

    #[test]
    fn render_wpa_conf_writes_exactly_one_network_block() {
        let creds = WifiCredentials {
            ssid: "Home".into(),
            password: "s3cret".into(),
        };
        let body = render_wpa_conf(&creds, "US");
        let count = body.matches("network={").count();
        assert_eq!(count, 1, "exactly one network block must be rendered");
        assert!(body.contains("ssid=\"Home\""));
        assert!(body.contains("psk=\"s3cret\""));
        assert!(body.contains("country=US\n"));
        assert!(body.contains("ctrl_interface=/var/run/wpa_supplicant\n"));
    }

    #[test]
    fn render_wpa_conf_escapes_password_with_quote() {
        let creds = WifiCredentials {
            ssid: "net".into(),
            password: "pa\"ss".into(),
        };
        let body = render_wpa_conf(&creds, "US");
        assert!(
            body.contains("psk=\"pa\\\"ss\""),
            "password with a quote must be escaped in the rendered config, got:\n{}",
            body
        );
        assert!(
            !body.contains("psk=\"pa\"ss\""),
            "un-escaped quote would break parsing"
        );
    }

    #[test]
    fn render_wpa_conf_uses_provided_country_code() {
        let creds = WifiCredentials {
            ssid: "net".into(),
            password: "p".into(),
        };
        assert!(render_wpa_conf(&creds, "DE").contains("country=DE\n"));
        assert!(render_wpa_conf(&creds, "JP").contains("country=JP\n"));
    }

    #[test]
    fn parse_wpa_credentials_reads_written_network() {
        let creds = WifiCredentials {
            ssid: "Guest Wi-Fi".into(),
            password: "pa\\\"ss".into(),
        };
        let body = render_wpa_conf(&creds, "US");

        assert_eq!(parse_wpa_credentials(&body, None), Some(creds));
    }

    #[test]
    fn parse_wpa_credentials_prefers_active_ssid() {
        let body = r#"
ctrl_interface=/var/run/wpa_supplicant
network={
  ssid="first"
  psk="wrong"
}
network={
  ssid="robnet"
  psk="correct"
}
"#;

        assert_eq!(
            parse_wpa_credentials(body, Some("robnet")),
            Some(WifiCredentials {
                ssid: "robnet".into(),
                password: "correct".into(),
            })
        );
        assert_eq!(parse_wpa_credentials(body, None), None);
    }

    #[test]
    fn parse_wpa_credentials_accepts_unquoted_psk() {
        let body = r#"
network={
  ssid="robnet"
  psk=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
}
"#;

        assert_eq!(
            parse_wpa_credentials(body, Some("robnet")),
            Some(WifiCredentials {
                ssid: "robnet".into(),
                password: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
            })
        );
    }

    #[test]
    fn parse_wpa_scan_results_filters_dedupes_and_sorts_networks() {
        let output = "bssid / frequency / signal level / flags / ssid\n\
11:22:33:44:55:66\t2412\t-66\t[WPA2-PSK-CCMP][ESS]\tKitchen\n\
22:33:44:55:66:77\t2412\t-42\t[WPA2-PSK-CCMP][ESS]\tKitchen\n\
33:44:55:66:77:88\t5180\t-52\t[WPA2-PSK+SAE-CCMP][ESS]\tStudio\n\
44:55:66:77:88:99\t2462\t-50\t[ESS]\tGuest\n\
55:66:77:88:99:aa\t2462\t-45\t[WEP][ESS]\tLegacy\n\
66:77:88:99:aa:bb\t2462\t-30\t[WPA2-PSK-CCMP][ESS]\t\n";

        let networks = parse_wpa_scan_results(output);

        assert_eq!(
            networks,
            vec![
                WifiScanNetwork {
                    ssid: "Kitchen".into(),
                    rssi: -42,
                    security: "wpa2".into(),
                    frequency: Some(2412),
                },
                WifiScanNetwork {
                    ssid: "Legacy".into(),
                    rssi: -45,
                    security: "wep".into(),
                    frequency: Some(2462),
                },
                WifiScanNetwork {
                    ssid: "Guest".into(),
                    rssi: -50,
                    security: "open".into(),
                    frequency: Some(2462),
                },
                WifiScanNetwork {
                    ssid: "Studio".into(),
                    rssi: -52,
                    security: "wpa3".into(),
                    frequency: Some(5180),
                },
            ]
        );
    }

    #[test]
    fn validate_credentials_accepts_typical_networks() {
        for (ssid, password) in [
            ("Home", "s3cret"),
            ("My Wi-Fi", "correct horse battery staple"),
            ("café 2.4GHz", "pa\"ss\\word"),
            ("x", ""),
        ] {
            let creds = WifiCredentials {
                ssid: ssid.into(),
                password: password.into(),
            };
            assert!(
                validate_credentials(&creds).is_ok(),
                "ssid={ssid:?} password={password:?} should validate"
            );
        }
    }

    #[test]
    fn validate_credentials_rejects_control_characters() {
        // A newline inside a quoted wpa value breaks the whole config file;
        // wpa_supplicant then refuses to start and Wi-Fi stays down.
        for (ssid, password) in [
            ("evil\nssid", "pass"),
            ("ssid", "pass\nword"),
            ("ssid\r", "pass"),
            ("ssid", "pass\tword"),
            ("ssid\0", "pass"),
        ] {
            let creds = WifiCredentials {
                ssid: ssid.into(),
                password: password.into(),
            };
            assert!(
                validate_credentials(&creds).is_err(),
                "ssid={ssid:?} password={password:?} must be rejected"
            );
        }
    }

    #[test]
    fn validate_credentials_rejects_empty_and_oversized_ssid() {
        let empty = WifiCredentials {
            ssid: "   ".into(),
            password: "pass".into(),
        };
        assert!(validate_credentials(&empty).is_err());

        let oversized = WifiCredentials {
            ssid: "x".repeat(MAX_SSID_BYTES + 1),
            password: "pass".into(),
        };
        assert!(validate_credentials(&oversized).is_err());

        let max = WifiCredentials {
            ssid: "x".repeat(MAX_SSID_BYTES),
            password: "pass".into(),
        };
        assert!(validate_credentials(&max).is_ok());
    }

    #[test]
    fn write_atomic_0600_replaces_content_without_leaving_tmp() {
        let dir = std::env::temp_dir().join(format!(
            "rhythm-wifi-atomic-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let conf = dir.join("wpa_supplicant.conf");

        write_atomic_0600(&conf, "first\n").unwrap();
        assert_eq!(fs::read_to_string(&conf).unwrap(), "first\n");

        write_atomic_0600(&conf, "second\n").unwrap();
        assert_eq!(fs::read_to_string(&conf).unwrap(), "second\n");

        assert!(
            !dir.join("wpa_supplicant.conf.tmp").exists(),
            "temp sibling must not survive a successful write"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&conf).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "credentials file must stay private");
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_atomic_0600_recovers_from_stale_tmp() {
        let dir = std::env::temp_dir().join(format!(
            "rhythm-wifi-stale-tmp-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let conf = dir.join("wpa_supplicant.conf");
        // Simulate a crash mid-write from a prior run.
        fs::write(dir.join("wpa_supplicant.conf.tmp"), "partial garbage").unwrap();

        write_atomic_0600(&conf, "clean\n").unwrap();

        assert_eq!(fs::read_to_string(&conf).unwrap(), "clean\n");
        assert!(!dir.join("wpa_supplicant.conf.tmp").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn render_wpa_conf_is_roundtrippable_through_simple_parser() {
        // Sanity check: every network-block line has balanced quotes.
        let creds = WifiCredentials {
            ssid: "Guest Wi-Fi".into(),
            password: "hunter2".into(),
        };
        let body = render_wpa_conf(&creds, "US");
        let in_network = body
            .lines()
            .skip_while(|line| !line.trim_start().starts_with("network={"))
            .skip(1)
            .take_while(|line| !line.trim_start().starts_with('}'));
        for line in in_network {
            let quote_count = line.matches('"').count();
            let escaped_quote_count = line.matches("\\\"").count();
            let effective_quotes = quote_count - escaped_quote_count;
            assert!(
                effective_quotes % 2 == 0,
                "line has unbalanced quotes: {}",
                line
            );
        }
    }
}
