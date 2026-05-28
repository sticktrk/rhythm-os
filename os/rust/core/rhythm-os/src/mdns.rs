/// mDNS constants shared across all Rhythm OS platforms.
pub const MDNS_SERVICE_TYPE: &str = "_http";
pub const MDNS_SERVICE_PROTO: &str = "_tcp";
pub const MDNS_INSTANCE_NAME: &str = "Rhythm OS";
pub const MDNS_HOSTNAME_PREFIX: &str = "rhythm-";
pub const MDNS_TXT_VERSION: &str = "version";
pub const MDNS_TXT_TYPE: &str = "type";

const MDNS_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
const MDNS_ANNOUNCE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

#[derive(Clone, Debug, Eq, PartialEq)]
struct LocalIpv4Interface {
    name: String,
    ip: std::net::Ipv4Addr,
}

/// Find the first non-loopback IPv4 address on this machine.
pub fn local_ipv4() -> Option<std::net::Ipv4Addr> {
    local_ipv4_interface().map(|iface| iface.ip)
}

fn local_ipv4_interface() -> Option<LocalIpv4Interface> {
    use std::net::IpAddr;
    let ifaces = if_addrs::get_if_addrs().ok()?;
    let candidates = ifaces.into_iter().filter_map(|iface| {
        if iface.is_loopback() {
            return None;
        }

        let IpAddr::V4(ip) = iface.addr.ip() else {
            return None;
        };

        Some(LocalIpv4Interface {
            name: iface.name,
            ip,
        })
    });

    select_local_ipv4_interface(candidates)
}

fn select_local_ipv4_interface<I>(ifaces: I) -> Option<LocalIpv4Interface>
where
    I: IntoIterator<Item = LocalIpv4Interface>,
{
    let mut preferred = None;
    let mut fallback = None;

    for iface in ifaces {
        if iface.name.starts_with("usb") {
            fallback.get_or_insert(iface);
            continue;
        }

        if iface.name.starts_with("wlan")
            || iface.name.starts_with("wl")
            || iface.name.starts_with("eth")
            || iface.name.starts_with("en")
        {
            preferred.get_or_insert(iface);
            continue;
        }

        fallback.get_or_insert(iface);
    }

    preferred.or(fallback)
}

/// Register an mDNS service for auto-discovery by clients.
///
/// Returns `Some(handle)` on success. Keep the returned handle alive for the
/// lifetime of the process to maintain and refresh the advertisement as the
/// local IPv4 changes.
pub fn register_mdns_service(
    port: u16,
    device_suffix: &str,
    version: &str,
    device_type: &str,
) -> Option<MdnsRegistrationHandle> {
    register_mdns_service_with_id(port, device_suffix, None, version, device_type)
}

/// Register an mDNS service with a stable device identifier (e.g., a serial
/// number or MAC). When provided, the identifier is mixed into the advertised
/// hostname so two units on the same LAN don't collide on the IP-derived
/// suffix alone. Use this from platform binaries that can obtain a stable
/// per-unit ID at startup.
pub fn register_mdns_service_with_id(
    port: u16,
    device_suffix: &str,
    device_id: Option<&str>,
    version: &str,
    device_type: &str,
) -> Option<MdnsRegistrationHandle> {
    use log::warn;
    use std::sync::mpsc;

    let config = MdnsRegistrationConfig {
        port,
        device_suffix: device_suffix.to_string(),
        device_id: device_id.map(str::to_string),
        version: version.to_string(),
        device_type: device_type.to_string(),
    };
    let (stop_tx, stop_rx) = mpsc::channel();
    let thread_name = format!("mdns-{}", device_suffix);
    let join_handle = match std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || run_mdns_registration_loop(config, stop_rx))
    {
        Ok(join_handle) => join_handle,
        Err(e) => {
            warn!(target: "sys", "mDNS: failed to spawn registrar thread: {:?}", e);
            return None;
        }
    };

    Some(MdnsRegistrationHandle {
        stop_tx: Some(stop_tx),
        join_handle: Some(join_handle),
    })
}

struct MdnsRegistrationConfig {
    port: u16,
    device_suffix: String,
    device_id: Option<String>,
    version: String,
    device_type: String,
}

/// Keeps desktop/Linux mDNS registration aligned with the current local IPv4.
pub struct MdnsRegistrationHandle {
    stop_tx: Option<std::sync::mpsc::Sender<()>>,
    join_handle: Option<std::thread::JoinHandle<()>>,
}

impl Drop for MdnsRegistrationHandle {
    fn drop(&mut self) {
        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(());
        }
        if let Some(join_handle) = self.join_handle.take() {
            let _ = join_handle.join();
        }
    }
}

fn run_mdns_registration_loop(
    config: MdnsRegistrationConfig,
    stop_rx: std::sync::mpsc::Receiver<()>,
) {
    use log::info;

    let mut advertised_interface = None;
    let mut daemon: Option<mdns_sd::ServiceDaemon> = None;
    let mut logged_waiting_for_ip = false;

    loop {
        if let Some(detected_interface) = local_ipv4_interface() {
            if advertised_interface.as_ref() != Some(&detected_interface) || daemon.is_none() {
                if let Some(old_interface) = advertised_interface.as_ref() {
                    if old_interface != &detected_interface {
                        info!(
                            target: "sys",
                            "mDNS: local IPv4 interface changed from {} ({}) to {} ({}), refreshing advertisement",
                            old_interface.ip,
                            old_interface.name,
                            detected_interface.ip,
                            detected_interface.name
                        );
                    }
                }
                if let Some(old_daemon) = daemon.take() {
                    let _ = old_daemon.shutdown();
                }

                match register_mdns_service_for_interface(
                    config.port,
                    &config.device_suffix,
                    config.device_id.as_deref(),
                    &config.version,
                    &config.device_type,
                    &detected_interface,
                ) {
                    Some(new_daemon) => {
                        daemon = Some(new_daemon);
                        advertised_interface = Some(detected_interface);
                        logged_waiting_for_ip = false;
                    }
                    None => {
                        daemon = None;
                        advertised_interface = None;
                    }
                }
            }
        } else {
            if !logged_waiting_for_ip {
                info!(
                    target: "sys",
                    "mDNS: waiting for a non-loopback IPv4 before advertising {}",
                    config.device_suffix
                );
                logged_waiting_for_ip = true;
            }
            if let Some(old_daemon) = daemon.take() {
                let _ = old_daemon.shutdown();
            }
            advertised_interface = None;
        }

        match stop_rx.recv_timeout(MDNS_POLL_INTERVAL) {
            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
    }

    if let Some(old_daemon) = daemon.take() {
        let _ = old_daemon.shutdown();
    }
}

fn register_mdns_service_for_interface(
    port: u16,
    device_suffix: &str,
    device_id: Option<&str>,
    version: &str,
    device_type: &str,
    interface: &LocalIpv4Interface,
) -> Option<mdns_sd::ServiceDaemon> {
    use log::{info, warn};
    use std::net::IpAddr;

    let service_type = format!("{}.{}.local.", MDNS_SERVICE_TYPE, MDNS_SERVICE_PROTO);
    let local_ip = interface.ip;
    let hostname = mdns_hostname_with_id(device_suffix, device_id, local_ip);
    let instance_name = format!("{} ({})", MDNS_INSTANCE_NAME, hostname);

    let daemon = match mdns_sd::ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            warn!(target: "sys", "mDNS: failed to start daemon: {:?}", e);
            return None;
        }
    };

    let monitor = match daemon.monitor() {
        Ok(monitor) => monitor,
        Err(e) => {
            warn!(target: "sys", "mDNS: failed to monitor daemon: {:?}", e);
            let _ = daemon.shutdown();
            return None;
        }
    };

    // Keep discovery scoped to the chosen LAN IPv4. Otherwise mdns-sd may also
    // publish on USB, VPN, or other host interfaces that clients cannot use.
    if let Err(e) = daemon.disable_interface(mdns_sd::IfKind::All) {
        warn!(target: "sys", "mDNS: failed to disable default interfaces: {:?}", e);
        let _ = daemon.shutdown();
        return None;
    }

    if let Err(e) = daemon.enable_interface(IpAddr::V4(local_ip)) {
        warn!(
            target: "sys",
            "mDNS: failed to enable interface {} ({}): {:?}",
            interface.name,
            local_ip,
            e
        );
        let _ = daemon.shutdown();
        return None;
    }

    let service_info = match mdns_service_info(
        &service_type,
        &instance_name,
        &hostname,
        port,
        version,
        device_type,
    ) {
        Ok(info) => info,
        Err(e) => {
            warn!(target: "sys", "mDNS: failed to create service info: {:?}", e);
            let _ = daemon.shutdown();
            return None;
        }
    };
    let service_fullname = service_info.get_fullname().to_string();

    if let Err(e) = daemon.register(service_info) {
        warn!(target: "sys", "mDNS: failed to register service: {:?}", e);
        let _ = daemon.shutdown();
        return None;
    }

    // register() only confirms the command was queued. Wait for the daemon's
    // Announce event so logs and health bundles do not claim LAN visibility
    // when no multicast socket was actually able to publish.
    let announced_on = match wait_for_mdns_announcement(&monitor, &service_fullname) {
        Ok(addrs) => addrs,
        Err(e) => {
            warn!(
                target: "sys",
                "mDNS: registered {} on {} ({}) but no LAN announcement was observed: {}; retrying",
                hostname,
                local_ip,
                interface.name,
                e
            );
            let _ = daemon.shutdown();
            return None;
        }
    };

    info!(
        target: "sys",
        "mDNS: advertising as {}.local ({}) on {} port {} via {}",
        hostname,
        local_ip,
        interface.name,
        port,
        announced_on
    );
    Some(daemon)
}

fn mdns_service_info(
    service_type: &str,
    instance_name: &str,
    hostname: &str,
    port: u16,
    version: &str,
    device_type: &str,
) -> mdns_sd::Result<mdns_sd::ServiceInfo> {
    mdns_sd::ServiceInfo::new(
        service_type,
        instance_name,
        &format!("{}.local.", hostname),
        (),
        port,
        [(MDNS_TXT_VERSION, version), (MDNS_TXT_TYPE, device_type)].as_slice(),
    )
    .map(mdns_sd::ServiceInfo::enable_addr_auto)
}

fn wait_for_mdns_announcement(
    monitor: &mdns_sd::Receiver<mdns_sd::DaemonEvent>,
    service_fullname: &str,
) -> Result<String, String> {
    let deadline = std::time::Instant::now() + MDNS_ANNOUNCE_TIMEOUT;
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match monitor.recv_timeout(remaining) {
            Ok(mdns_sd::DaemonEvent::Announce(fullname, addrs)) => {
                if fullname == service_fullname {
                    return Ok(addrs);
                }
            }
            Ok(mdns_sd::DaemonEvent::Error(e)) => return Err(format!("{:?}", e)),
            Ok(mdns_sd::DaemonEvent::IpAdd(_)) | Ok(mdns_sd::DaemonEvent::IpDel(_)) => {}
            Ok(_) => {}
            Err(e) => return Err(e.to_string()),
        }
    }

    Err("timed out waiting for mdns-sd announce event".to_string())
}

/// Compose the mDNS hostname. When a stable `device_id` is provided
/// (sanitized serial / MAC), it is preferred — the resulting hostname stays
/// unique across reboots and DHCP churn even if two units land on the same
/// /24 subnet. When no `device_id` is available, the IP-derived suffix is
/// used as a legacy fallback (pre-P1.12 behavior).
pub fn mdns_hostname_with_id(
    device_suffix: &str,
    device_id: Option<&str>,
    local_ip: std::net::Ipv4Addr,
) -> String {
    match device_id
        .map(sanitize_device_id_component)
        .filter(|s| !s.is_empty())
    {
        Some(id) => format!("{}{}-{}", MDNS_HOSTNAME_PREFIX, device_suffix, id),
        None => {
            let octets = local_ip.octets();
            format!(
                "{}{}-{:02x}{:02x}",
                MDNS_HOSTNAME_PREFIX, device_suffix, octets[2], octets[3]
            )
        }
    }
}

/// Produce a hostname-safe lowercase component by keeping ASCII alphanumerics
/// and collapsing everything else. Ensures the result fits mDNS hostname
/// rules (letters, digits, hyphens only) and doesn't accidentally include
/// characters that break DNS or mDNS parsing.
fn sanitize_device_id_component(raw: &str) -> String {
    let trimmed: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(16)
        .collect();
    trimmed.to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn iface(name: &str, ip: [u8; 4]) -> LocalIpv4Interface {
        LocalIpv4Interface {
            name: name.to_string(),
            ip: Ipv4Addr::from(ip),
        }
    }

    #[test]
    fn local_ipv4_selection_prefers_lan_interfaces_over_usb() {
        let selected = select_local_ipv4_interface([
            iface("usb0", [169, 254, 1, 10]),
            iface("docker0", [172, 17, 0, 1]),
            iface("wlan0", [192, 168, 0, 13]),
        ]);

        assert_eq!(selected, Some(iface("wlan0", [192, 168, 0, 13])));
    }

    #[test]
    fn local_ipv4_selection_keeps_interface_name_with_ip() {
        let selected = select_local_ipv4_interface([
            iface("en0", [192, 168, 0, 214]),
            iface("wlan0", [192, 168, 0, 13]),
        ]);

        assert_eq!(selected, Some(iface("en0", [192, 168, 0, 214])));
    }

    #[test]
    fn mdns_service_info_uses_addr_auto_for_selected_interface_registration() {
        let info = mdns_service_info(
            "_http._tcp.local.",
            "Rhythm OS (rhythm-server-31810e88)",
            "rhythm-server-31810e88",
            54448,
            "0.4.241-beta",
            "rhythm-server",
        )
        .expect("valid service info");

        assert!(
            info.is_addr_auto(),
            "mdns-sd should populate addresses from the selected daemon interface"
        );
        assert!(
            info.get_addresses().is_empty(),
            "service info must not carry a stale fixed IP before daemon registration"
        );
        assert_eq!(info.get_hostname(), "rhythm-server-31810e88.local.");
        assert_eq!(info.get_port(), 54448);
        assert_eq!(
            info.get_property_val_str(MDNS_TXT_VERSION),
            Some("0.4.241-beta")
        );
        assert_eq!(
            info.get_property_val_str(MDNS_TXT_TYPE),
            Some("rhythm-server")
        );
    }

    #[test]
    fn mdns_hostname_uses_suffix_and_last_two_octets() {
        assert_eq!(
            mdns_hostname_with_id("server", None, Ipv4Addr::new(192, 168, 4, 27)),
            "rhythm-server-041b"
        );
    }

    #[test]
    fn mdns_hostname_prefers_device_id_over_ip_suffix() {
        assert_eq!(
            mdns_hostname_with_id("server", Some("ABC12345"), Ipv4Addr::new(192, 168, 4, 27)),
            "rhythm-server-abc12345",
            "a stable device_id must be used instead of the IP-derived suffix"
        );
    }

    #[test]
    fn mdns_hostname_falls_back_to_ip_when_device_id_empty() {
        assert_eq!(
            mdns_hostname_with_id("server", Some(""), Ipv4Addr::new(192, 168, 4, 27)),
            "rhythm-server-041b"
        );
    }

    #[test]
    fn mdns_hostname_sanitizes_non_alphanumeric_characters_in_device_id() {
        assert_eq!(
            mdns_hostname_with_id(
                "appliance",
                Some("AA:BB:CC:DD:EE:FF"),
                Ipv4Addr::new(10, 0, 0, 1)
            ),
            "rhythm-appliance-aabbccddeeff",
            "colons in MAC addresses must be stripped"
        );
        assert_eq!(
            mdns_hostname_with_id(
                "appliance",
                Some("rpiz/serial\0\u{0}"),
                Ipv4Addr::new(10, 0, 0, 1)
            ),
            "rhythm-appliance-rpizserial",
            "slash, null, and control chars must be stripped"
        );
    }

    #[test]
    fn mdns_hostname_truncates_long_device_ids() {
        let long = "0123456789ABCDEF0123456789ABCDEF";
        let host = mdns_hostname_with_id("appliance", Some(long), Ipv4Addr::new(10, 0, 0, 1));
        assert!(
            host.len() <= "rhythm-appliance-".len() + 16,
            "device_id must be truncated to prevent overlong hostnames, got {}",
            host
        );
    }

    #[test]
    fn two_units_with_different_device_ids_get_distinct_hostnames() {
        let ip = Ipv4Addr::new(192, 168, 1, 100);
        let a = mdns_hostname_with_id("appliance", Some("UNIT-0001"), ip);
        let b = mdns_hostname_with_id("appliance", Some("UNIT-0002"), ip);
        assert_ne!(a, b, "distinct device IDs must produce distinct hostnames");
    }

    #[test]
    fn sanitize_device_id_accepts_hex_and_alphanumeric() {
        assert_eq!(sanitize_device_id_component("abc123"), "abc123");
        assert_eq!(sanitize_device_id_component("ABCDEF"), "abcdef");
    }

    #[test]
    fn sanitize_device_id_drops_empty_after_cleaning() {
        assert_eq!(sanitize_device_id_component(":::"), "");
        assert_eq!(sanitize_device_id_component(""), "");
    }
}
