//! Linux appliance Wi-Fi helpers for rpiz provisioning and recovery.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use rhythm_os::provisioning::WifiCredentials;

const WPA_CONF: &str = "/etc/wpa_supplicant.conf";
const WIFI_INIT_SCRIPT: &str = "/etc/init.d/S42wifi";
const WPA_CLI: &str = "/usr/sbin/wpa_cli";
const DEFAULT_COUNTRY: &str = "US";

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

pub fn connect_with_credentials(creds: &WifiCredentials, timeout: Duration) -> Result<String> {
    write_wifi_credentials(creds)?;
    restart_wifi()?;
    wait_for_ip(timeout)
}

pub fn clear_credentials_and_restart() -> Result<()> {
    clear_wifi_credentials()?;
    restart_wifi()
}

pub fn clear_credentials() -> Result<()> {
    clear_wifi_credentials()
}

fn write_wifi_credentials(creds: &WifiCredentials) -> Result<()> {
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
    if let Some(parent) = Path::new(WPA_CONF).parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(WPA_CONF, body).with_context(|| format!("writing {}", WPA_CONF))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(WPA_CONF, perms).with_context(|| format!("chmod 600 {}", WPA_CONF))?;
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
    let status = Command::new(WIFI_INIT_SCRIPT)
        .arg(action)
        .status()
        .with_context(|| format!("running {} {}", WIFI_INIT_SCRIPT, action))?;
    if !status.success() {
        bail!(
            "{} {} failed with status {}",
            WIFI_INIT_SCRIPT,
            action,
            status
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
    let output = Command::new(WPA_CLI)
        .args(["-i", iface, "status"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().find_map(|line| {
        line.strip_prefix(&format!("{field}="))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
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
        .or_else(|| match networks.as_slice() {
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
