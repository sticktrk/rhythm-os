//! Typed Matter controller transport abstraction.
//!
//! The transport boundary is intentionally shaped around the light operations
//! Rhythm actually needs, not around raw cluster/TLV plumbing. Current
//! server-class builds map this to the official CHIP controller stack through
//! a native daemon; future board-specific targets can provide their own
//! controller implementation later.

use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Default minimum interval for Matter attribute subscriptions.
pub const DEFAULT_SUBSCRIPTION_MIN_INTERVAL_SECS: u16 = 1;
/// Default maximum interval for Matter attribute subscriptions.
pub const DEFAULT_SUBSCRIPTION_MAX_INTERVAL_SECS: u16 = 60;

/// An ordered operation in a desired-state update for one Matter endpoint.
///
/// The complete plan is submitted to the controller sidecar as one owned
/// job. This keeps command ordering and recovery below the application RPC
/// boundary instead of making Rhythm OS synchronously shepherd each cluster
/// command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum MatterCommandStep {
    SetOnOff {
        on: bool,
    },
    Identify {
        duration_secs: u16,
    },
    SetBrightness {
        level: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transition_ms: Option<u32>,
    },
    RunLevel {
        command: MatterLevelCommandVariant,
        level_or_step: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step_mode: Option<MatterLevelStepMode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transition_ms: Option<u32>,
    },
    SetColorTemperature {
        kelvin: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transition_ms: Option<u32>,
    },
    SetXy {
        x: f32,
        y: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transition_ms: Option<u32>,
    },
    SetHueSaturation {
        hue: u8,
        saturation: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transition_ms: Option<u32>,
    },
}

/// Latest desired state for one Matter endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatterEndpointCommandPlan {
    /// Stable application-generated identifier used for terminal outcomes.
    pub command_id: u64,
    pub node_id: u64,
    pub endpoint: u16,
    pub steps: Vec<MatterCommandStep>,
    /// Device-profile pacing between steps. This is not retry backoff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inter_step_delay_ms: Option<u64>,
}

/// How a transport handled a command plan submission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatterCommandSubmission {
    pub command_id: u64,
    /// `true` for compatibility transports which completed inline. Native
    /// chipd returns `false` and publishes a terminal controller event later.
    pub completed: bool,
    /// Identifies the chipd process that owns an accepted command. This lets
    /// the OS distinguish commands accepted after a sidecar restart from
    /// indeterminate work owned by the previous process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller_stream_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatterCommandOutcomeStatus {
    Succeeded,
    Failed,
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatterCommandOutcome {
    pub command_id: u64,
    pub node_id: u64,
    pub endpoint: u16,
    pub status: MatterCommandOutcomeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum MatterControllerEvent {
    CommandOutcome(MatterCommandOutcome),
    AttributeReport(MatterAttributeReport),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatterControllerEventEnvelope {
    pub sequence: u64,
    pub event: MatterControllerEvent,
}

/// Cursor into one chipd process's event stream. A changed stream id tells the
/// client that the sidecar restarted and in-flight outcomes were lost.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatterControllerEventCursor {
    pub stream_id: String,
    pub sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatterControllerEventBatch {
    pub stream_id: String,
    /// Oldest sequence still retained by the sidecar. A cursor below this
    /// value has a gap and its pending commands must be treated as unknown.
    pub oldest_sequence: u64,
    pub events: Vec<MatterControllerEventEnvelope>,
}

/// Matter network type for commissioning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatterCommissioningNetwork {
    /// Matter-over-WiFi.
    Wifi,
}

/// Rendezvous method used during commissioning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatterCommissioningRendezvous {
    /// Let the backend choose the best supported flow for the device.
    Auto,
    /// Force BLE rendezvous.
    Ble,
    /// Force on-network rendezvous.
    OnNetwork,
}

/// Appliance Wi-Fi credentials used for Matter accessory commissioning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatterCommissioningWifiCredentials {
    pub ssid: String,
    pub password: String,
}

/// Shared typed commissioning request built by the orchestrator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatterCommissionRequest {
    /// Raw Matter setup payload. May be a QR payload (`MT:`) or a manual code.
    pub setup_payload: String,
    /// Matter node ID to assign on our fabric.
    pub node_id: u64,
    /// Matter network type being commissioned.
    pub network: MatterCommissioningNetwork,
    /// Rendezvous method used to reach the device.
    pub rendezvous: MatterCommissioningRendezvous,
    /// Stored appliance Wi-Fi credentials used during commissioning.
    pub wifi_credentials: MatterCommissioningWifiCredentials,
}

/// Platform-agnostic typed interface to a Matter light controller.
pub trait MatterTransport: Send + Sync {
    /// Submit complete desired-state plans for controller-owned execution.
    ///
    /// Compatibility transports execute inline. Native chipd overrides this
    /// method to accept immediately, coalesce by endpoint, and publish terminal
    /// outcomes through [`Self::wait_controller_events`].
    fn submit_endpoint_plans(
        &self,
        plans: &[MatterEndpointCommandPlan],
    ) -> Result<Vec<MatterCommandSubmission>> {
        let results = std::thread::scope(|scope| {
            plans
                .iter()
                .map(|plan| {
                    scope.spawn(move || -> Result<MatterCommandSubmission> {
                        for (index, step) in plan.steps.iter().enumerate() {
                            self.execute_command_step(plan.node_id, plan.endpoint, step)?;
                            if index + 1 < plan.steps.len() {
                                if let Some(delay_ms) =
                                    plan.inter_step_delay_ms.filter(|delay| *delay > 0)
                                {
                                    std::thread::sleep(Duration::from_millis(delay_ms));
                                }
                            }
                        }
                        Ok(MatterCommandSubmission {
                            command_id: plan.command_id,
                            completed: true,
                            controller_stream_id: None,
                        })
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .map_err(|_| anyhow::anyhow!("Matter endpoint plan worker panicked"))?
                })
                .collect::<Vec<_>>()
        });
        let mut submissions = Vec::with_capacity(results.len());
        let mut first_error = None;
        for result in results {
            match result {
                Ok(submission) => submissions.push(submission),
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(submissions),
        }
    }

    /// Wait for controller events newer than `cursor`. The default keeps
    /// additive compatibility with transports that have no event stream.
    fn wait_controller_events(
        &self,
        cursor: Option<&MatterControllerEventCursor>,
        max_wait: Duration,
    ) -> Result<MatterControllerEventBatch> {
        if !max_wait.is_zero() {
            std::thread::sleep(max_wait);
        }
        Ok(MatterControllerEventBatch {
            stream_id: cursor
                .map(|cursor| cursor.stream_id.clone())
                .unwrap_or_else(|| "inline".to_string()),
            oldest_sequence: cursor.map(|cursor| cursor.sequence + 1).unwrap_or(1),
            events: Vec::new(),
        })
    }

    /// Execute one plan step for the synchronous compatibility path.
    fn execute_command_step(
        &self,
        node_id: u64,
        endpoint: u16,
        step: &MatterCommandStep,
    ) -> Result<()> {
        match *step {
            MatterCommandStep::SetOnOff { on } => self.set_on_off(node_id, endpoint, on),
            MatterCommandStep::Identify { duration_secs } => {
                self.identify_light(node_id, endpoint, duration_secs)
            }
            MatterCommandStep::SetBrightness {
                level,
                transition_ms,
            } => self.set_brightness(node_id, endpoint, level, transition_ms),
            MatterCommandStep::RunLevel {
                command,
                level_or_step,
                step_mode,
                transition_ms,
            } => self.run_level_command(
                node_id,
                endpoint,
                command,
                level_or_step,
                step_mode,
                transition_ms,
            ),
            MatterCommandStep::SetColorTemperature {
                kelvin,
                transition_ms,
            } => self.set_color_temperature(node_id, endpoint, kelvin, transition_ms),
            MatterCommandStep::SetXy {
                x,
                y,
                transition_ms,
            } => self.set_xy(node_id, endpoint, x, y, transition_ms),
            MatterCommandStep::SetHueSaturation {
                hue,
                saturation,
                transition_ms,
            } => self.set_hue_saturation(node_id, endpoint, hue, saturation, transition_ms),
        }
    }

    /// Commission a light and return the fully probed device description.
    fn commission_light(&self, request: &MatterCommissionRequest) -> Result<CommissionedDevice>;

    /// Remove a device from the local fabric.
    fn decommission_device(&self, node_id: u64, force: bool) -> Result<()>;

    /// List devices currently known to the controller.
    fn list_devices(&self) -> Result<Vec<MatterDeviceInfo>>;

    /// List the complete device records persisted by the controller.
    ///
    /// Older transports may not support this additive contract. Callers must
    /// fall back to [`Self::list_devices`] without probing when it is absent.
    fn list_commissioned_devices(&self) -> Result<Vec<CommissionedDevice>> {
        anyhow::bail!("persisted Matter device records are not supported by this transport")
    }

    /// Probe a single node and return its typed light capabilities.
    fn probe_light(&self, node_id: u64) -> Result<CommissionedDevice>;

    /// Set the On/Off state of a light endpoint.
    fn set_on_off(&self, node_id: u64, endpoint: u16, on: bool) -> Result<()>;

    /// Create/update a Matter group and add the requested member endpoints.
    fn configure_group(&self, group: &MatterGroup) -> Result<()> {
        let _ = group;
        anyhow::bail!("Matter groups are not supported by this transport")
    }

    /// Remove a Matter group from the local controller and member endpoints.
    fn remove_group(&self, group_id: u16, members: &[MatterGroupMember]) -> Result<()> {
        let _ = (group_id, members);
        anyhow::bail!("Matter groups are not supported by this transport")
    }

    /// Set the On/Off state of a Matter group.
    fn set_group_on_off(&self, group_id: u16, on: bool) -> Result<()> {
        let _ = (group_id, on);
        anyhow::bail!("Matter group On/Off is not supported by this transport")
    }

    /// Ask a Matter group to identify itself for the given number of seconds.
    fn identify_group(&self, group_id: u16, duration_secs: u16) -> Result<()> {
        let _ = (group_id, duration_secs);
        anyhow::bail!("Matter group Identify is not supported by this transport")
    }

    /// Set a Matter group level using Matter's 0-254 level encoding.
    fn set_group_brightness(
        &self,
        group_id: u16,
        level: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let _ = (group_id, level, transition_ms);
        anyhow::bail!("Matter group Level Control is not supported by this transport")
    }

    /// Set a Matter group color temperature in Kelvin.
    fn set_group_color_temperature(
        &self,
        group_id: u16,
        kelvin: u16,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let _ = (group_id, kelvin, transition_ms);
        anyhow::bail!("Matter group Color Control is not supported by this transport")
    }

    /// Set a Matter group CIE xy color.
    fn set_group_xy(
        &self,
        group_id: u16,
        x: f32,
        y: f32,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let _ = (group_id, x, y, transition_ms);
        anyhow::bail!("Matter group Color Control is not supported by this transport")
    }

    /// Set a Matter group hue/saturation color using Matter's 0-254 encoding.
    fn set_group_hue_saturation(
        &self,
        group_id: u16,
        hue: u8,
        saturation: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let _ = (group_id, hue, saturation, transition_ms);
        anyhow::bail!("Matter group Color Control is not supported by this transport")
    }

    /// Ask a light endpoint to identify itself for the given number of seconds.
    fn identify_light(&self, node_id: u64, endpoint: u16, duration_secs: u16) -> Result<()>;

    /// Set a light level using Matter's 0-254 level encoding.
    fn set_brightness(
        &self,
        node_id: u64,
        endpoint: u16,
        level: u8,
        transition_ms: Option<u32>,
    ) -> Result<()>;

    /// Run a specific Matter Level Control command variant.
    fn run_level_command(
        &self,
        node_id: u64,
        endpoint: u16,
        command: MatterLevelCommandVariant,
        level_or_step: u8,
        step_mode: Option<MatterLevelStepMode>,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let _ = (
            node_id,
            endpoint,
            command,
            level_or_step,
            step_mode,
            transition_ms,
        );
        anyhow::bail!("Matter Level Control command variants are not supported by this transport")
    }

    /// Set a color temperature in Kelvin.
    fn set_color_temperature(
        &self,
        node_id: u64,
        endpoint: u16,
        kelvin: u16,
        transition_ms: Option<u32>,
    ) -> Result<()>;

    /// Set a CIE xy color.
    fn set_xy(
        &self,
        node_id: u64,
        endpoint: u16,
        x: f32,
        y: f32,
        transition_ms: Option<u32>,
    ) -> Result<()>;

    /// Set a hue/saturation color using Matter's 0-254 encoding.
    fn set_hue_saturation(
        &self,
        node_id: u64,
        endpoint: u16,
        hue: u8,
        saturation: u8,
        transition_ms: Option<u32>,
    ) -> Result<()>;

    /// Read the On/Off state from a light endpoint.
    fn read_on_off(&self, node_id: u64, endpoint: u16) -> Result<bool>;

    /// Read a raw capability snapshot for a Matter light endpoint.
    fn read_light_capability_snapshot(&self, node_id: u64, endpoint: u16) -> Result<Value> {
        let _ = (node_id, endpoint);
        anyhow::bail!("Matter light capability snapshots are not supported by this transport")
    }

    /// Read the current light attributes available to this transport.
    fn read_light_state(&self, node_id: u64, endpoint: u16) -> Result<Value> {
        let _ = (node_id, endpoint);
        anyhow::bail!("Matter light state snapshots are not supported by this transport")
    }

    /// Subscribe to On/Off attribute reports for the given light endpoints.
    fn subscribe_on_off(
        &self,
        targets: &[MatterSubscriptionTarget],
        min_interval_secs: u16,
        max_interval_secs: u16,
    ) -> Result<()> {
        let _ = (targets, min_interval_secs, max_interval_secs);
        anyhow::bail!("Matter On/Off attribute subscriptions are not supported by this transport")
    }

    /// Drain queued attribute reports from a subscription-capable transport.
    fn drain_attribute_reports(&self) -> Result<Vec<MatterAttributeReport>> {
        Ok(Vec::new())
    }
}

/// A Matter endpoint whose attributes should be subscribed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatterSubscriptionTarget {
    pub node_id: u64,
    pub endpoint: u16,
}

/// A single Matter endpoint that belongs to a group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatterGroupMember {
    pub node_id: u64,
    pub endpoint: u16,
}

/// Matter group configuration known by the local controller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatterGroup {
    pub group_id: u16,
    pub name: String,
    pub members: Vec<MatterGroupMember>,
}

/// A typed Matter attribute value carried over the local transport.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum MatterAttributeValue {
    Bool(bool),
}

/// A raw attribute report from a Matter subscription.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatterAttributeReport {
    /// Source device node ID.
    pub node_id: u64,
    /// Endpoint the report came from.
    pub endpoint: u16,
    /// Cluster ID.
    pub cluster: u32,
    /// Attribute ID.
    pub attr_id: u32,
    /// Decoded attribute value.
    pub value: MatterAttributeValue,
}

/// A light that has been commissioned into the local Matter fabric.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommissionedDevice {
    /// Matter node ID assigned during commissioning.
    pub node_id: u64,
    /// Device vendor name (from Basic Information cluster).
    pub vendor_name: String,
    /// Device product name (from Basic Information cluster).
    pub product_name: String,
    /// Vendor ID.
    pub vendor_id: u16,
    /// Product ID.
    pub product_id: u16,
    /// Serial number (if reported by device).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial_number: Option<String>,
    /// Light endpoint (typically 1).
    pub light_endpoint: u16,
    /// Supported color modes discovered during probing.
    pub color_modes: Vec<MatterColorMode>,
    /// Color temperature range in Kelvin (if CT supported).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_kelvin: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_kelvin: Option<u16>,
}

/// Matter color modes (from the Color Control cluster).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatterColorMode {
    HueSaturation,
    Xy,
    ColorTemperature,
}

/// Matter Level Control command variants used by bulb profiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatterLevelCommandVariant {
    MoveToLevel,
    MoveToLevelWithOnOff,
    Step,
    StepWithOnOff,
}

/// Matter Level Control step direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatterLevelStepMode {
    Up,
    Down,
}

/// Basic device info returned by `MatterTransport::list_devices()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatterDeviceInfo {
    pub node_id: u64,
    pub vendor_name: String,
    pub product_name: String,
    pub reachable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MinimalMatterTransport;

    impl MatterTransport for MinimalMatterTransport {
        fn commission_light(
            &self,
            request: &MatterCommissionRequest,
        ) -> Result<CommissionedDevice> {
            Ok(device(request.node_id))
        }

        fn decommission_device(&self, _node_id: u64, _force: bool) -> Result<()> {
            Ok(())
        }

        fn list_devices(&self) -> Result<Vec<MatterDeviceInfo>> {
            Ok(vec![MatterDeviceInfo {
                node_id: 1,
                vendor_name: "Vendor".to_string(),
                product_name: "Lamp".to_string(),
                reachable: true,
            }])
        }

        fn probe_light(&self, node_id: u64) -> Result<CommissionedDevice> {
            Ok(device(node_id))
        }

        fn set_on_off(&self, _node_id: u64, _endpoint: u16, _on: bool) -> Result<()> {
            Ok(())
        }

        fn identify_light(&self, _node_id: u64, _endpoint: u16, _duration_secs: u16) -> Result<()> {
            Ok(())
        }

        fn set_brightness(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _level: u8,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn set_color_temperature(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _kelvin: u16,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn set_xy(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _x: f32,
            _y: f32,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn set_hue_saturation(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _hue: u8,
            _saturation: u8,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn read_on_off(&self, _node_id: u64, _endpoint: u16) -> Result<bool> {
            Ok(false)
        }
    }

    fn device(node_id: u64) -> CommissionedDevice {
        CommissionedDevice {
            node_id,
            vendor_name: "Vendor".to_string(),
            product_name: "Lamp".to_string(),
            vendor_id: 1,
            product_id: 2,
            serial_number: None,
            light_endpoint: 1,
            color_modes: vec![MatterColorMode::ColorTemperature],
            min_kelvin: Some(2700),
            max_kelvin: Some(5000),
        }
    }

    fn assert_error(result: Result<()>, expected: &str) {
        assert_eq!(result.unwrap_err().to_string(), expected);
    }

    #[test]
    fn default_group_and_snapshot_methods_report_unsupported() {
        let transport = MinimalMatterTransport;
        let group = MatterGroup {
            group_id: 1,
            name: "Kitchen".to_string(),
            members: vec![MatterGroupMember {
                node_id: 1,
                endpoint: 1,
            }],
        };

        assert_eq!(
            transport
                .list_commissioned_devices()
                .unwrap_err()
                .to_string(),
            "persisted Matter device records are not supported by this transport"
        );
        assert_error(
            transport.configure_group(&group),
            "Matter groups are not supported by this transport",
        );
        assert_error(
            transport.remove_group(1, &group.members),
            "Matter groups are not supported by this transport",
        );
        assert_error(
            transport.set_group_on_off(1, true),
            "Matter group On/Off is not supported by this transport",
        );
        assert_error(
            transport.identify_group(1, 3),
            "Matter group Identify is not supported by this transport",
        );
        assert_error(
            transport.set_group_brightness(1, 128, Some(100)),
            "Matter group Level Control is not supported by this transport",
        );
        assert_error(
            transport.set_group_color_temperature(1, 2700, None),
            "Matter group Color Control is not supported by this transport",
        );
        assert_error(
            transport.set_group_xy(1, 0.1, 0.2, None),
            "Matter group Color Control is not supported by this transport",
        );
        assert_error(
            transport.set_group_hue_saturation(1, 10, 20, None),
            "Matter group Color Control is not supported by this transport",
        );
        assert_error(
            transport.run_level_command(
                1,
                1,
                MatterLevelCommandVariant::Step,
                1,
                Some(MatterLevelStepMode::Down),
                None,
            ),
            "Matter Level Control command variants are not supported by this transport",
        );
        assert_eq!(
            transport
                .read_light_capability_snapshot(1, 1)
                .unwrap_err()
                .to_string(),
            "Matter light capability snapshots are not supported by this transport"
        );
        assert_eq!(
            transport.read_light_state(1, 1).unwrap_err().to_string(),
            "Matter light state snapshots are not supported by this transport"
        );
        assert_eq!(
            transport
                .subscribe_on_off(
                    &[MatterSubscriptionTarget {
                        node_id: 1,
                        endpoint: 1,
                    }],
                    DEFAULT_SUBSCRIPTION_MIN_INTERVAL_SECS,
                    DEFAULT_SUBSCRIPTION_MAX_INTERVAL_SECS,
                )
                .unwrap_err()
                .to_string(),
            "Matter On/Off attribute subscriptions are not supported by this transport"
        );
        assert!(transport.drain_attribute_reports().unwrap().is_empty());
    }

    #[test]
    fn request_and_value_types_round_trip_through_json() {
        let request = MatterCommissionRequest {
            setup_payload: "MT:payload".to_string(),
            node_id: 42,
            network: MatterCommissioningNetwork::Wifi,
            rendezvous: MatterCommissioningRendezvous::Ble,
            wifi_credentials: MatterCommissioningWifiCredentials {
                ssid: "Rhythm".to_string(),
                password: "secret".to_string(),
            },
        };

        let json = serde_json::to_string(&request).unwrap();
        let decoded: MatterCommissionRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, request);

        let report = MatterAttributeReport {
            node_id: 42,
            endpoint: 1,
            cluster: 6,
            attr_id: 0,
            value: MatterAttributeValue::Bool(true),
        };
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["value"]["type"], "bool");
        assert_eq!(json["value"]["value"], true);
        let decoded: MatterAttributeReport = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, report);

        let transport = MinimalMatterTransport;
        assert_eq!(transport.list_devices().unwrap()[0].node_id, 1);
        assert_eq!(transport.commission_light(&request).unwrap().node_id, 42);
        assert_eq!(transport.probe_light(9).unwrap().node_id, 9);
        assert!(!transport.read_on_off(1, 1).unwrap());
    }
}
