/// mDNS constants shared across all Rhythm OS platforms.
pub const MDNS_SERVICE_TYPE: &str = "_http";
pub const MDNS_SERVICE_PROTO: &str = "_tcp";
pub const MDNS_INSTANCE_NAME: &str = "Rhythm OS";
pub const MDNS_HOSTNAME_PREFIX: &str = "rhythm-";
pub const MDNS_TXT_VERSION: &str = "version";
pub const MDNS_TXT_TYPE: &str = "type";

const MDNS_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Find the first non-loopback IPv4 address on this machine.
pub fn local_ipv4() -> Option<std::net::Ipv4Addr> {
    use std::net::IpAddr;
    let ifaces = if_addrs::get_if_addrs().ok()?;

    let mut preferred = None;
    let mut fallback = None;

    for iface in ifaces {
        if iface.is_loopback() {
            continue;
        }

        let IpAddr::V4(ip) = iface.addr.ip() else {
            continue;
        };

        if iface.name.starts_with("usb") {
            fallback.get_or_insert(ip);
            continue;
        }

        if iface.name.starts_with("wlan")
            || iface.name.starts_with("wl")
            || iface.name.starts_with("eth")
            || iface.name.starts_with("en")
        {
            preferred.get_or_insert(ip);
            continue;
        }

        fallback.get_or_insert(ip);
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
    use log::warn;
    use std::sync::mpsc;

    let config = MdnsRegistrationConfig {
        port,
        device_suffix: device_suffix.to_string(),
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

    let mut advertised_ip = None;
    let mut daemon: Option<mdns_sd::ServiceDaemon> = None;
    let mut logged_waiting_for_ip = false;

    loop {
        if let Some(detected_ip) = local_ipv4() {
            if advertised_ip != Some(detected_ip) || daemon.is_none() {
                if let Some(old_ip) = advertised_ip {
                    if old_ip != detected_ip {
                        info!(
                            target: "sys",
                            "mDNS: local IPv4 changed from {} to {}, refreshing advertisement",
                            old_ip,
                            detected_ip
                        );
                    }
                }
                if let Some(old_daemon) = daemon.take() {
                    let _ = old_daemon.shutdown();
                }

                match register_mdns_service_for_ip(
                    config.port,
                    &config.device_suffix,
                    &config.version,
                    &config.device_type,
                    detected_ip,
                ) {
                    Some(new_daemon) => {
                        daemon = Some(new_daemon);
                        advertised_ip = Some(detected_ip);
                        logged_waiting_for_ip = false;
                    }
                    None => {
                        daemon = None;
                        advertised_ip = None;
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
            advertised_ip = None;
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

fn register_mdns_service_for_ip(
    port: u16,
    device_suffix: &str,
    version: &str,
    device_type: &str,
    local_ip: std::net::Ipv4Addr,
) -> Option<mdns_sd::ServiceDaemon> {
    use log::{info, warn};

    let service_type = format!("{}.{}.local.", MDNS_SERVICE_TYPE, MDNS_SERVICE_PROTO);
    let hostname = mdns_hostname(device_suffix, local_ip);
    let instance_name = format!("{} ({})", MDNS_INSTANCE_NAME, hostname);

    let daemon = match mdns_sd::ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            warn!(target: "sys", "mDNS: failed to start daemon: {:?}", e);
            return None;
        }
    };

    let service_info = match mdns_sd::ServiceInfo::new(
        &service_type,
        &instance_name,
        &format!("{}.local.", hostname),
        std::net::IpAddr::V4(local_ip),
        port,
        [(MDNS_TXT_VERSION, version), (MDNS_TXT_TYPE, device_type)].as_slice(),
    ) {
        Ok(info) => info,
        Err(e) => {
            warn!(target: "sys", "mDNS: failed to create service info: {:?}", e);
            let _ = daemon.shutdown();
            return None;
        }
    };

    if let Err(e) = daemon.register(service_info) {
        warn!(target: "sys", "mDNS: failed to register service: {:?}", e);
        let _ = daemon.shutdown();
        return None;
    }

    info!(
        target: "sys",
        "mDNS: advertising as {}.local ({}) on port {}",
        hostname,
        local_ip,
        port
    );
    Some(daemon)
}

fn mdns_hostname(device_suffix: &str, local_ip: std::net::Ipv4Addr) -> String {
    let octets = local_ip.octets();
    format!(
        "{}{}-{:02x}{:02x}",
        MDNS_HOSTNAME_PREFIX, device_suffix, octets[2], octets[3]
    )
}

#[cfg(test)]
mod tests {
    use super::mdns_hostname;
    use std::net::Ipv4Addr;

    #[test]
    fn mdns_hostname_uses_suffix_and_last_two_octets() {
        assert_eq!(
            mdns_hostname("server", Ipv4Addr::new(192, 168, 4, 27)),
            "rhythm-server-041b"
        );
    }
}
