//! Matter event translation.
//!
//! Translates Matter subscription attribute reports into `HubEvent` variants.
//! Matter devices report state changes via subscription reports rather than
//! SSE or WebSocket events.

use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_os::hub::HubEvent;

use crate::clusters;
use crate::lifecycle::format_device_id;

/// A raw attribute report from a Matter subscription.
#[derive(Debug, Clone)]
pub struct MatterAttributeReport {
    /// Source device node ID.
    pub node_id: u64,
    /// Endpoint the report came from.
    pub endpoint: u16,
    /// Cluster ID.
    pub cluster: u16,
    /// Attribute ID.
    pub attr_id: u16,
    /// Raw attribute value.
    pub value: Vec<u8>,
}

/// Translate a Matter attribute report into a HubEvent.
///
/// Currently only translates On/Off state changes. Level and color temperature
/// changes are not surfaced as events (Rhythm drives those, not the device).
pub fn translate_report(report: &MatterAttributeReport) -> Option<HubEvent> {
    match (report.cluster, report.attr_id) {
        (clusters::CLUSTER_ON_OFF, clusters::ATTR_ON_OFF) => {
            // On/Off state changed — this could indicate someone used a
            // physical switch or another controller. We emit DevicePaired
            // for now; future work may add a DeviceStateChanged event.
            let is_on = report.value.first().copied().unwrap_or(0) != 0;
            log::debug!(
                target: "evt",
                "Matter: node {} on/off changed to {}",
                report.node_id, is_on
            );
            // No direct HubEvent for state changes yet — log only.
            None
        }
        _ => None,
    }
}

/// Create a DevicePaired event for a newly commissioned device.
pub fn device_paired_event(
    node_id: u64,
    vendor_name: &str,
    product_name: &str,
) -> HubEvent {
    HubEvent::DevicePaired {
        hub_key: None, // Will be tagged by with_hub_key()
        device_id: format_device_id(node_id, 1),
        name: format!("{} {}", vendor_name, product_name),
        device_type: DeviceType::Light,
    }
}
