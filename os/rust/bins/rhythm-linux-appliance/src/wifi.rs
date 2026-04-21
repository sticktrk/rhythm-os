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
    write_wpa_conf(&body)
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
