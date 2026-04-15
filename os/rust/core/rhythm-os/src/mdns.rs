/// mDNS constants shared across all Rhythm OS platforms.
pub const MDNS_SERVICE_TYPE: &str = "_http";
pub const MDNS_SERVICE_PROTO: &str = "_tcp";
pub const MDNS_INSTANCE_NAME: &str = "Rhythm OS";
pub const MDNS_HOSTNAME_PREFIX: &str = "rhythm-";
pub const MDNS_TXT_VERSION: &str = "version";
pub const MDNS_TXT_TYPE: &str = "type";

/// Find the first non-loopback IPv4 address on this machine.
#[cfg(feature = "desktop")]
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
/// Returns `Some(daemon)` on success (keep alive to maintain registration),
/// or `None` if mDNS setup fails (non-fatal).
#[cfg(feature = "desktop")]
pub fn register_mdns_service(
    port: u16,
    device_suffix: &str,
    version: &str,
    device_type: &str,
) -> Option<mdns_sd::ServiceDaemon> {
    use log::{info, warn};

    let service_type = format!("{}.{}.local.", MDNS_SERVICE_TYPE, MDNS_SERVICE_PROTO);

    let local_ip = local_ipv4().unwrap_or(std::net::Ipv4Addr::LOCALHOST);
    let octets = local_ip.octets();

    let hostname = format!(
        "{}{}-{:02x}{:02x}",
        MDNS_HOSTNAME_PREFIX, device_suffix, octets[2], octets[3]
    );
    let instance_name = format!("{} ({})", MDNS_INSTANCE_NAME, hostname);

    let daemon = match mdns_sd::ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            warn!(target: "sys", "mDNS: failed to start daemon: {:?}", e);
            return None;
        }
    };

    let service_info = mdns_sd::ServiceInfo::new(
        &service_type,
        &instance_name,
        &format!("{}.local.", hostname),
        std::net::IpAddr::V4(local_ip),
        port,
        [(MDNS_TXT_VERSION, version), (MDNS_TXT_TYPE, device_type)].as_slice(),
    );

    match service_info {
        Ok(info) => {
            if let Err(e) = daemon.register(info) {
                warn!(target: "sys", "mDNS: failed to register service: {:?}", e);
            } else {
                info!(target: "sys", "mDNS: advertising as {}.local ({}) on port {}", hostname, local_ip, port);
            }
        }
        Err(e) => warn!(target: "sys", "mDNS: failed to create service info: {:?}", e),
    }

    Some(daemon)
}
