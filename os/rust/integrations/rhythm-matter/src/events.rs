//! Matter event translation.
//!
//! Translates Matter subscription attribute reports into `HubEvent` variants.
//! Matter devices report state changes via subscription reports rather than
//! SSE or WebSocket events.

use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_os::hub::{HubCommandOutcomeStatus, HubEvent};

use crate::clusters;
use crate::lifecycle::format_device_id;
use crate::transport::{
    MatterAttributeReport, MatterAttributeValue, MatterCommandOutcome, MatterCommandOutcomeStatus,
};

pub fn translate_command_outcome(
    controller_stream_id: String,
    outcome: MatterCommandOutcome,
) -> HubEvent {
    HubEvent::CommandOutcome {
        hub_key: None,
        controller_stream_id,
        command_id: outcome.command_id,
        device_id: format_device_id(outcome.node_id, outcome.endpoint),
        status: match outcome.status {
            MatterCommandOutcomeStatus::Succeeded => HubCommandOutcomeStatus::Succeeded,
            MatterCommandOutcomeStatus::Failed => HubCommandOutcomeStatus::Failed,
            MatterCommandOutcomeStatus::Superseded => HubCommandOutcomeStatus::Superseded,
        },
        detail: outcome.detail,
    }
}

/// Translate a Matter attribute report into a HubEvent.
///
/// Currently only translates On/Off state changes. Level and color temperature
/// changes are not surfaced as events (Rhythm drives those, not the device).
pub fn translate_report(report: &MatterAttributeReport) -> Option<HubEvent> {
    match (report.cluster, report.attr_id) {
        (clusters::CLUSTER_ON_OFF_U32, clusters::ATTR_ON_OFF_U32) => {
            let MatterAttributeValue::Bool(is_on) = &report.value;
            let is_on = *is_on;
            log::debug!(
                target: "evt",
                "Matter: node {} on/off changed to {}",
                report.node_id, is_on
            );
            Some(HubEvent::LightPower {
                hub_key: None,
                device_id: format_device_id(report.node_id, report.endpoint),
                lights_on: is_on,
            })
        }
        _ => None,
    }
}

/// Create a DevicePaired event for a newly commissioned device.
pub fn device_paired_event(node_id: u64, vendor_name: &str, product_name: &str) -> HubEvent {
    HubEvent::DevicePaired {
        hub_key: None, // Will be tagged by with_hub_key()
        device_id: format_device_id(node_id, 1),
        name: format!("{} {}", vendor_name, product_name),
        device_type: DeviceType::Light,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_on_off_report_to_light_power_event() {
        let event = translate_report(&MatterAttributeReport {
            node_id: 42,
            endpoint: 2,
            cluster: clusters::CLUSTER_ON_OFF_U32,
            attr_id: clusters::ATTR_ON_OFF_U32,
            value: MatterAttributeValue::Bool(true),
        })
        .expect("on/off report should translate");

        match event {
            HubEvent::LightPower {
                device_id,
                lights_on,
                ..
            } => {
                assert_eq!(device_id, "matter-42-2");
                assert!(lights_on);
            }
            other => panic!("unexpected event: {:?}", other),
        }
    }
}
