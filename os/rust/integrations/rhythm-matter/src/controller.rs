//! Matter light controller using the typed Matter transport.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use async_trait::async_trait;
use log::{debug, info, warn};
use rhythm_core::controller::{
    HubCommandReceipt, HubDispatchTarget, HubLightController, LightControlError,
    LightControlResult, LightController,
};
use rhythm_core::lighting::LightingCommand;
use rhythm_core::room::Room;
#[cfg(test)]
use rhythm_devices::LightType;
use rhythm_devices::{ColorPreference, DeviceQuirk, LightCapabilities};

use crate::clusters;
use crate::hub_state::MatterHubData;
use crate::transport::{MatterCommandStep, MatterEndpointCommandPlan, MatterTransport};

/// Type alias for the registry (same as HA and Hue).
pub type MatterDeviceRegistry = rhythm_os::registry::HubDeviceRegistry;

const DISPATCH_INFO_MS: u128 = 250;
const DISPATCH_WARN_MS: u128 = 1000;
const MATTER_IDENTIFY_DURATION_SECS: u16 = 1;
const MATTER_GROUP_CONTROL_PREFIX: &str = "matter-group-";
const MATTER_GROUP_FANOUT_ONLY_ENV: &str = "RHYTHM_MATTER_GROUP_FANOUT_ONLY";
const MATTER_GROUP_SAFETY_FANOUT_ENV: &str = "RHYTHM_MATTER_GROUP_SAFETY_FANOUT";
const MATTER_ON_OFF_READ_BACKOFF: Duration = Duration::from_secs(120);

fn env_flag_enabled(name: &str, default: bool) -> bool {
    std::env::var(name)
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            !matches!(normalized.as_str(), "0" | "false" | "off" | "no")
        })
        .unwrap_or(default)
}

pub(crate) fn matter_group_fanout_only_enabled() -> bool {
    env_flag_enabled(MATTER_GROUP_FANOUT_ONLY_ENV, true)
}

#[derive(Clone, Copy, Debug)]
struct MatterOnOffReadBackoff {
    marked_at: Instant,
    suppress_until: Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MatterOnOffRead {
    On,
    Off,
    Suppressed,
}

fn initial_connectivity_backoff(
    hub_data: &MatterHubData,
) -> HashMap<(u64, u16), MatterOnOffReadBackoff> {
    let marked_at = Instant::now();
    let suppress_until = marked_at + MATTER_ON_OFF_READ_BACKOFF;
    hub_data
        .commissioned
        .lock()
        .map(|commissioned| {
            commissioned
                .iter()
                .filter(|device| !device.reachable)
                .map(|device| {
                    (
                        (device.node_id, 1),
                        MatterOnOffReadBackoff {
                            marked_at,
                            suppress_until,
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Format the hub-native control ID used for a Matter group.
pub fn format_group_control_id(group_id: u16) -> String {
    format!("{}{}", MATTER_GROUP_CONTROL_PREFIX, group_id)
}

/// Parse a Matter group control ID.
pub fn parse_group_control_id(control_id: &str) -> Option<u16> {
    control_id
        .strip_prefix(MATTER_GROUP_CONTROL_PREFIX)
        .and_then(|group_id| group_id.parse::<u16>().ok())
        .filter(|group_id| *group_id != 0)
}

/// Light controller implementation using typed Matter light operations.
pub struct MatterLightController {
    transport: Arc<dyn MatterTransport>,
    hub_data: Arc<MatterHubData>,
    on_off_read_backoff: Mutex<HashMap<(u64, u16), MatterOnOffReadBackoff>>,
    group_fanout_only: bool,
    next_command_id: AtomicU64,
}

impl MatterLightController {
    /// Create a new Matter light controller.
    pub fn new(transport: Arc<dyn MatterTransport>, hub_data: Arc<MatterHubData>) -> Self {
        let on_off_read_backoff = Mutex::new(initial_connectivity_backoff(&hub_data));
        Self {
            transport,
            hub_data,
            on_off_read_backoff,
            group_fanout_only: matter_group_fanout_only_enabled(),
            next_command_id: AtomicU64::new(
                chrono::Utc::now().timestamp_millis().unsigned_abs().max(1) << 16,
            ),
        }
    }

    fn next_command_id(&self) -> u64 {
        self.next_command_id.fetch_add(1, Ordering::Relaxed).max(1)
    }

    fn submit_plans(
        &self,
        target_label: &str,
        plans: Vec<MatterEndpointCommandPlan>,
    ) -> LightControlResult<HubCommandReceipt> {
        if plans.is_empty() {
            return Ok(HubCommandReceipt::delivered());
        }
        let expected_ids: Vec<u64> = plans.iter().map(|plan| plan.command_id).collect();
        let submissions = match self.transport.submit_endpoint_plans(&plans) {
            Ok(submissions) => submissions,
            Err(error) => {
                if Self::looks_like_connectivity_timeout(&error) {
                    for plan in &plans {
                        self.mark_connectivity_failed(plan.node_id, plan.endpoint);
                    }
                }
                return Err(LightControlError::CommandFailed(format!(
                    "Matter controller rejected target {} plans: {error:#}",
                    target_label
                )));
            }
        };
        let returned_ids: Vec<u64> = submissions
            .iter()
            .map(|submission| submission.command_id)
            .collect();
        if returned_ids != expected_ids {
            return Err(LightControlError::CommandFailed(format!(
                "Matter controller returned mismatched command ids for target {}",
                target_label
            )));
        }
        for (plan, submission) in plans.iter().zip(&submissions) {
            if submission.completed {
                self.clear_connectivity_backoff(plan.node_id, plan.endpoint);
            }
        }
        let pending_ids: Vec<u64> = submissions
            .iter()
            .filter(|submission| !submission.completed)
            .map(|submission| submission.command_id)
            .collect();
        if pending_ids.is_empty() {
            Ok(HubCommandReceipt::delivered())
        } else {
            let mut stream_ids = submissions
                .iter()
                .filter(|submission| !submission.completed)
                .filter_map(|submission| submission.controller_stream_id.as_deref());
            let Some(stream_id) = stream_ids.next() else {
                return Err(LightControlError::CommandFailed(format!(
                    "Matter controller accepted target {} plans without a stream identity",
                    target_label
                )));
            };
            if stream_ids.any(|candidate| candidate != stream_id) {
                return Err(LightControlError::CommandFailed(format!(
                    "Matter controller accepted target {} plans across multiple stream identities",
                    target_label
                )));
            }
            Ok(HubCommandReceipt::accepted(
                pending_ids,
                stream_id.to_string(),
            ))
        }
    }

    fn turn_on_plans(
        &self,
        device_ids: &[String],
        command: &LightingCommand,
    ) -> LightControlResult<Vec<MatterEndpointCommandPlan>> {
        let mut plans = Vec::with_capacity(device_ids.len());
        for device_id in device_ids {
            let Some((node_id, endpoint)) = Self::parse_device_id(device_id) else {
                warn!(target: "cmd", "Matter: invalid device ID format: {}", device_id);
                continue;
            };
            let (caps, quirks) = self.device_metadata(device_id, node_id);
            let adapted = rhythm_os::controller_helpers::adapt_lighting_command(
                &caps,
                command,
                Self::color_preference(&quirks),
            );
            let mut steps = Vec::new();
            let needs_explicit_on = adapted.on && Self::needs_explicit_on(&quirks);
            if needs_explicit_on {
                steps.push(MatterCommandStep::SetOnOff { on: true });
            }
            if let Some((hue, saturation)) = adapted.hue_saturation {
                steps.push(MatterCommandStep::SetHueSaturation {
                    hue,
                    saturation,
                    transition_ms: adapted.transition_ms,
                });
            } else if let Some((x, y)) = adapted.xy {
                steps.push(MatterCommandStep::SetXy {
                    x,
                    y,
                    transition_ms: adapted.transition_ms,
                });
            } else if let Some(kelvin) = adapted.kelvin {
                steps.push(MatterCommandStep::SetColorTemperature {
                    kelvin,
                    transition_ms: adapted.transition_ms,
                });
            }
            if let Some(brightness) = adapted.brightness {
                steps.push(MatterCommandStep::SetBrightness {
                    level: clusters::brightness_to_level(brightness),
                    transition_ms: adapted.transition_ms,
                });
            } else if adapted.on && !needs_explicit_on {
                steps.push(MatterCommandStep::SetOnOff { on: true });
            }
            if steps.is_empty() {
                continue;
            }
            plans.push(MatterEndpointCommandPlan {
                command_id: self.next_command_id(),
                node_id,
                endpoint,
                steps,
                inter_step_delay_ms: quirks.iter().find_map(|quirk| match quirk {
                    DeviceQuirk::CommandThrottleMs(ms) if *ms > 0 => Some(u64::from(*ms)),
                    _ => None,
                }),
            });
        }
        if plans.is_empty() && !device_ids.is_empty() {
            return Err(LightControlError::CommandFailed(
                "Matter target has no valid endpoint plans".to_string(),
            ));
        }
        Ok(plans)
    }

    fn turn_off_plans(
        &self,
        device_ids: &[String],
    ) -> LightControlResult<Vec<MatterEndpointCommandPlan>> {
        let plans: Vec<_> = device_ids
            .iter()
            .filter_map(|device_id| {
                let Some((node_id, endpoint)) = Self::parse_device_id(device_id) else {
                    warn!(target: "cmd", "Matter: invalid device ID format: {}", device_id);
                    return None;
                };
                Some(MatterEndpointCommandPlan {
                    command_id: self.next_command_id(),
                    node_id,
                    endpoint,
                    steps: vec![MatterCommandStep::SetOnOff { on: false }],
                    inter_step_delay_ms: None,
                })
            })
            .collect();
        if plans.is_empty() && !device_ids.is_empty() {
            return Err(LightControlError::CommandFailed(
                "Matter target has no valid endpoint plans".to_string(),
            ));
        }
        Ok(plans)
    }

    fn submit_turn_on_devices(
        &self,
        target_label: &str,
        device_ids: &[String],
        command: &LightingCommand,
    ) -> LightControlResult<HubCommandReceipt> {
        if device_ids.is_empty() {
            return Err(LightControlError::CommandFailed(format!(
                "Matter target {target_label} has no member endpoints"
            )));
        }
        self.submit_plans(target_label, self.turn_on_plans(device_ids, command)?)
    }

    fn submit_turn_off_devices(
        &self,
        target_label: &str,
        device_ids: &[String],
    ) -> LightControlResult<HubCommandReceipt> {
        if device_ids.is_empty() {
            return Err(LightControlError::CommandFailed(format!(
                "Matter target {target_label} has no member endpoints"
            )));
        }
        self.submit_plans(target_label, self.turn_off_plans(device_ids)?)
    }

    /// Parse a Matter device ID string into `(node_id, endpoint)`.
    ///
    /// Format: `matter-{node_id}` or `matter-{node_id}-{endpoint}`.
    fn parse_device_id(device_id: &str) -> Option<(u64, u16)> {
        let parts: Vec<&str> = device_id.strip_prefix("matter-")?.split('-').collect();
        let node_id = parts.first()?.parse::<u64>().ok()?;
        let endpoint = parts
            .get(1)
            .and_then(|endpoint| endpoint.parse::<u16>().ok())
            .unwrap_or(1);
        Some((node_id, endpoint))
    }

    /// Resolve the concrete Matter device IDs for a registry-backed room target.
    fn target_device_ids(&self, room_id: &str) -> LightControlResult<Vec<String>> {
        let registry =
            self.hub_data.registry.lock().map_err(|e| {
                LightControlError::Internal(format!("Failed to lock registry: {}", e))
            })?;

        let device_ids = registry.get_light_entities(room_id);
        if !device_ids.is_empty() || registry.get_grouped_light_id(room_id).is_some() {
            return Ok(device_ids);
        }

        Err(LightControlError::RoomNotFound(format!(
            "No target for room {}",
            room_id
        )))
    }

    fn target_device_ids_for_target(
        &self,
        target: &HubDispatchTarget,
    ) -> LightControlResult<Vec<String>> {
        match target {
            HubDispatchTarget::Group { room_id, .. } => self.target_device_ids(room_id),
            HubDispatchTarget::Devices { native_ids } => Ok(native_ids.clone()),
        }
    }

    fn group_id_for_target(target: &HubDispatchTarget) -> Option<u16> {
        match target {
            HubDispatchTarget::Group { control_id, .. } => parse_group_control_id(control_id),
            HubDispatchTarget::Devices { .. } => None,
        }
    }

    fn group_fallback_target_label(target_label: &str, group_id: u16) -> String {
        format!("{target_label} fallback-from-group-{group_id}")
    }

    fn group_safety_fanout_enabled() -> bool {
        env_flag_enabled(MATTER_GROUP_SAFETY_FANOUT_ENV, true)
    }

    fn log_group_fanout_only(
        operation: &'static str,
        target_label: &str,
        fallback_target_label: &str,
        group_id: u16,
        member_count: usize,
    ) {
        tracing::info!(
            target: "cmd",
            event = "matter_group_fanout_only",
            operation,
            target = %target_label,
            fallback_target = %fallback_target_label,
            group_id,
            member_count,
            reason = "groupcast_delivery_unverified",
            disable_env = MATTER_GROUP_FANOUT_ONLY_ENV,
            "Matter group fan-out-only mode"
        );
    }

    fn log_group_safety_fanout(
        operation: &'static str,
        target_label: &str,
        fallback_target_label: &str,
        group_id: u16,
        member_count: usize,
        group_commands_ok: usize,
        group_commands_failed: usize,
    ) {
        tracing::info!(
            target: "cmd",
            event = "matter_group_safety_fanout",
            operation,
            target = %target_label,
            fallback_target = %fallback_target_label,
            group_id,
            member_count,
            group_commands_ok,
            group_commands_failed,
            reason = "groupcast_delivery_unverified",
            disable_env = MATTER_GROUP_SAFETY_FANOUT_ENV,
            "Matter group command safety fan-out"
        );
    }

    fn device_metadata(
        &self,
        device_id: &str,
        node_id: u64,
    ) -> (LightCapabilities, Vec<DeviceQuirk>) {
        let cached_caps = self
            .hub_data
            .device_caps
            .lock()
            .ok()
            .and_then(|device_caps| device_caps.get(device_id).cloned());
        let cached_quirks = self
            .hub_data
            .device_quirks
            .lock()
            .ok()
            .and_then(|device_quirks| device_quirks.get(device_id).cloned());

        if let (Some(caps), Some(quirks)) = (cached_caps, cached_quirks) {
            return (caps, quirks);
        }

        match self.transport.probe_light(node_id) {
            Ok(device) => {
                let mut caps = crate::commissioning::build_device_capabilities(&device);
                let mut quirks = crate::commissioning::build_device_quirks(&device);
                if let Ok(cloud_profiles) = self.hub_data.cloud_profiles.lock() {
                    cloud_profiles.apply_to_device(&device, &mut caps, &mut quirks);
                }
                let probed_id =
                    crate::lifecycle::format_device_id(device.node_id, device.light_endpoint);

                self.cache_device_metadata(device_id, &probed_id, &caps, &quirks);
                if let Err(error) = crate::capture::persist_device_capture(
                    &self.hub_data,
                    &device,
                    "on_demand_probe",
                ) {
                    warn!(
                        target: "cmd",
                        "Matter: failed to persist on-demand probe capture for {}: {}",
                        probed_id,
                        error
                    );
                }

                debug!(
                    target: "cmd",
                    "Matter: probed device metadata on demand for {}",
                    device_id
                );
                (caps, quirks)
            }
            Err(error) => {
                warn!(
                    target: "cmd",
                    "Matter: failed to probe metadata for {} (node {}): {}; using extended-color fallback",
                    device_id,
                    node_id,
                    error
                );
                (
                    crate::commissioning::fallback_device_capabilities(),
                    Vec::new(),
                )
            }
        }
    }

    fn cache_device_metadata(
        &self,
        requested_id: &str,
        probed_id: &str,
        caps: &LightCapabilities,
        quirks: &[DeviceQuirk],
    ) {
        if let Ok(mut device_caps) = self.hub_data.device_caps.lock() {
            device_caps.insert(requested_id.to_string(), caps.clone());
            if requested_id != probed_id {
                device_caps.insert(probed_id.to_string(), caps.clone());
            }
        }

        if let Ok(mut device_quirks) = self.hub_data.device_quirks.lock() {
            device_quirks.insert(requested_id.to_string(), quirks.to_vec());
            if requested_id != probed_id {
                device_quirks.insert(probed_id.to_string(), quirks.to_vec());
            }
        }
    }

    fn color_preference(quirks: &[DeviceQuirk]) -> ColorPreference {
        if quirks
            .iter()
            .any(|quirk| matches!(quirk, DeviceQuirk::NeedsHueSaturationNotCt))
        {
            ColorPreference::PreferHueSaturation
        } else if quirks
            .iter()
            .any(|quirk| matches!(quirk, DeviceQuirk::NeedsXyNotCt))
        {
            ColorPreference::PreferXy
        } else {
            ColorPreference::PreferColorTemperature
        }
    }

    fn needs_explicit_on(quirks: &[DeviceQuirk]) -> bool {
        quirks
            .iter()
            .any(|quirk| matches!(quirk, DeviceQuirk::NeedsExplicitOn))
    }

    fn clear_connectivity_backoff(&self, node_id: u64, endpoint: u16) {
        if let Ok(mut backoff) = self.on_off_read_backoff.lock() {
            backoff.remove(&(node_id, endpoint));
        }
        self.hub_data.record_node_proof_of_life(node_id);
    }

    fn mark_connectivity_failed(&self, node_id: u64, endpoint: u16) {
        let marked_at = Instant::now();
        if let Ok(mut backoff) = self.on_off_read_backoff.lock() {
            backoff.insert(
                (node_id, endpoint),
                MatterOnOffReadBackoff {
                    marked_at,
                    suppress_until: marked_at + MATTER_ON_OFF_READ_BACKOFF,
                },
            );
        }
        self.hub_data.mark_node_reachable(node_id, false);
    }

    fn read_backoff_active(&self, node_id: u64, endpoint: u16) -> bool {
        let now = Instant::now();
        if let Ok(mut backoff) = self.on_off_read_backoff.lock() {
            match backoff.get(&(node_id, endpoint)).copied() {
                Some(entry) if entry.suppress_until > now => {
                    if self
                        .hub_data
                        .has_node_proof_of_life_after(node_id, entry.marked_at)
                    {
                        backoff.remove(&(node_id, endpoint));
                        return false;
                    }
                    return true;
                }
                Some(_) => {
                    backoff.remove(&(node_id, endpoint));
                }
                None => {}
            }
        }
        false
    }

    fn note_connectivity_failure(
        &self,
        node_id: u64,
        endpoint: u16,
        error: &anyhow::Error,
    ) -> bool {
        let connectivity_timeout = Self::looks_like_connectivity_timeout(error);
        if connectivity_timeout {
            self.mark_connectivity_failed(node_id, endpoint);
        }
        connectivity_timeout
    }

    fn read_on_off_with_backoff(&self, node_id: u64, endpoint: u16) -> Result<MatterOnOffRead> {
        if self.read_backoff_active(node_id, endpoint) {
            return Ok(MatterOnOffRead::Suppressed);
        }

        match self.transport.read_on_off(node_id, endpoint) {
            Ok(true) => {
                self.clear_connectivity_backoff(node_id, endpoint);
                Ok(MatterOnOffRead::On)
            }
            Ok(false) => {
                self.clear_connectivity_backoff(node_id, endpoint);
                Ok(MatterOnOffRead::Off)
            }
            Err(e) => {
                self.note_connectivity_failure(node_id, endpoint, &e);
                Err(e)
            }
        }
    }

    fn looks_like_connectivity_timeout(error: &anyhow::Error) -> bool {
        error.chain().any(|cause| {
            let lower = cause.to_string().to_ascii_lowercase();
            lower.contains("timeout")
                || lower.contains("timed out")
                || lower.contains("chip error 0x32")
                || lower.contains("failed to connect")
                || cause.downcast_ref::<std::io::Error>().is_some_and(|error| {
                    matches!(
                        error.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    )
                })
        })
    }

    fn identify_devices(
        &self,
        target_label: &str,
        device_ids: &[String],
    ) -> LightControlResult<()> {
        let started = Instant::now();
        let mut successful_devices = 0usize;
        let mut failed_devices = 0usize;

        for device_id in device_ids {
            let Some((node_id, endpoint)) = Self::parse_device_id(device_id) else {
                warn!(target: "cmd", "Matter: invalid device ID format: {}", device_id);
                failed_devices += 1;
                continue;
            };

            if let Err(e) =
                self.transport
                    .identify_light(node_id, endpoint, MATTER_IDENTIFY_DURATION_SECS)
            {
                warn!(
                    target: "cmd",
                    "Matter: identify command failed for node {}: {}",
                    node_id,
                    e
                );
                failed_devices += 1;
                self.note_connectivity_failure(node_id, endpoint, &e);
            } else {
                successful_devices += 1;
                self.clear_connectivity_backoff(node_id, endpoint);
            }
        }

        if successful_devices == 0 && !device_ids.is_empty() {
            return Err(LightControlError::CommandFailed(format!(
                "Matter identify failed for target {} ({} target devices)",
                target_label, failed_devices,
            )));
        }

        let latency_ms = started.elapsed().as_millis();
        if failed_devices > 0 {
            tracing::warn!(
                target: "cmd",
                event = "matter_identify_partial",
                target = %target_label,
                latency_ms,
                ok = successful_devices,
                failed = failed_devices,
                device_count = device_ids.len(),
                "Matter identify partial"
            );
        } else if latency_ms >= DISPATCH_WARN_MS {
            tracing::warn!(
                target: "cmd",
                event = "matter_identify",
                target = %target_label,
                latency_ms,
                device_count = device_ids.len(),
                "Matter identify slow"
            );
        } else if latency_ms >= DISPATCH_INFO_MS {
            tracing::info!(
                target: "cmd",
                event = "matter_identify",
                target = %target_label,
                latency_ms,
                device_count = device_ids.len(),
                "Matter identify"
            );
        } else {
            debug!(
                target: "cmd",
                "Matter identify: target={} devices={} latency_ms={}",
                target_label,
                device_ids.len(),
                latency_ms
            );
        }

        Ok(())
    }

    fn identify_group(
        &self,
        target_label: &str,
        group_id: u16,
        device_ids: &[String],
    ) -> LightControlResult<()> {
        if self.group_fanout_only {
            if device_ids.is_empty() {
                return Err(LightControlError::CommandFailed(format!(
                    "Matter group identify fan-out-only mode has no members for target {} (group {})",
                    target_label, group_id,
                )));
            }

            let fallback_target_label = Self::group_fallback_target_label(target_label, group_id);
            Self::log_group_fanout_only(
                "identify",
                target_label,
                &fallback_target_label,
                group_id,
                device_ids.len(),
            );
            return self.identify_devices(&fallback_target_label, device_ids);
        }

        let started = Instant::now();
        info!(
            target: "cmd",
            "Matter group identify dispatch: target={} group_id={} duration_secs={} command=Identify.Identify",
            target_label,
            group_id,
            MATTER_IDENTIFY_DURATION_SECS
        );
        if let Err(e) = self
            .transport
            .identify_group(group_id, MATTER_IDENTIFY_DURATION_SECS)
        {
            if device_ids.is_empty() {
                return Err(LightControlError::CommandFailed(format!(
                    "Matter identify failed for target {} (group {}, no fallback members): {}",
                    target_label, group_id, e
                )));
            }

            let fallback_target_label = Self::group_fallback_target_label(target_label, group_id);
            tracing::warn!(
                target: "cmd",
                event = "matter_group_identify_fallback",
                target = %target_label,
                fallback_target = %fallback_target_label,
                group_id,
                member_count = device_ids.len(),
                error = %e,
                "Matter group identify failed; falling back to member fan-out"
            );
            return self.identify_devices(&fallback_target_label, device_ids);
        }

        let latency_ms = started.elapsed().as_millis();
        if latency_ms >= DISPATCH_WARN_MS {
            tracing::warn!(
                target: "cmd",
                event = "matter_group_identify",
                target = %target_label,
                group_id,
                latency_ms,
                "Matter group identify slow"
            );
        } else if latency_ms >= DISPATCH_INFO_MS {
            tracing::info!(
                target: "cmd",
                event = "matter_group_identify",
                target = %target_label,
                group_id,
                latency_ms,
                "Matter group identify"
            );
        } else {
            info!(
                target: "cmd",
                "Matter group identify sent: target={} group_id={} latency_ms={}",
                target_label, group_id, latency_ms
            );
        }

        if Self::group_safety_fanout_enabled() && !device_ids.is_empty() {
            let fallback_target_label = Self::group_fallback_target_label(target_label, group_id);
            Self::log_group_safety_fanout(
                "identify",
                target_label,
                &fallback_target_label,
                group_id,
                device_ids.len(),
                1,
                0,
            );
            return self.identify_devices(&fallback_target_label, device_ids);
        }

        Ok(())
    }
}

#[async_trait]
impl HubLightController for MatterLightController {
    async fn turn_on_target(
        &self,
        target: &HubDispatchTarget,
        command: LightingCommand,
    ) -> LightControlResult<()> {
        self.turn_on_target_with_receipt(target, command).await?;
        Ok(())
    }

    async fn turn_off_target(
        &self,
        target: &HubDispatchTarget,
        _transition_ms: Option<u32>,
    ) -> LightControlResult<()> {
        self.turn_off_target_with_receipt(target, _transition_ms)
            .await?;
        Ok(())
    }

    async fn turn_on_target_with_receipt(
        &self,
        target: &HubDispatchTarget,
        command: LightingCommand,
    ) -> LightControlResult<HubCommandReceipt> {
        let target_label = target.label();
        let device_ids = self.target_device_ids_for_target(target)?;
        if device_ids.is_empty() {
            return Err(LightControlError::CommandFailed(format!(
                "Matter target {target_label} has no member endpoints"
            )));
        }
        self.submit_turn_on_devices(&target_label, &device_ids, &command)
    }

    async fn turn_off_target_with_receipt(
        &self,
        target: &HubDispatchTarget,
        _transition_ms: Option<u32>,
    ) -> LightControlResult<HubCommandReceipt> {
        let target_label = target.label();
        let device_ids = self.target_device_ids_for_target(target)?;
        if device_ids.is_empty() {
            return Err(LightControlError::CommandFailed(format!(
                "Matter target {target_label} has no member endpoints"
            )));
        }
        self.submit_turn_off_devices(&target_label, &device_ids)
    }

    async fn flash_target(&self, target: &HubDispatchTarget) -> LightControlResult<()> {
        let target_label = target.label();
        let device_ids = self.target_device_ids_for_target(target)?;
        if let Some(group_id) = Self::group_id_for_target(target) {
            return self.identify_group(&target_label, group_id, &device_ids);
        }
        self.identify_devices(&target_label, &device_ids)
    }

    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        rhythm_os::controller_helpers::rooms_from_registry(&self.hub_data.registry)
    }

    async fn is_connected(&self) -> bool {
        self.transport.list_devices().is_ok()
    }

    async fn any_lights_on_target(&self, target: &HubDispatchTarget) -> LightControlResult<bool> {
        let device_ids = self.target_device_ids_for_target(target)?;

        if device_ids.is_empty() {
            return Ok(false);
        }

        for device_id in &device_ids {
            let Some((node_id, endpoint)) = Self::parse_device_id(device_id) else {
                continue;
            };
            match self.read_on_off_with_backoff(node_id, endpoint) {
                Ok(MatterOnOffRead::On) => return Ok(true),
                Ok(MatterOnOffRead::Off) | Ok(MatterOnOffRead::Suppressed) => {}
                Err(e) => {
                    let connectivity_timeout = Self::looks_like_connectivity_timeout(&e);
                    warn!(
                        target: "cmd",
                        "Matter: failed to read on/off state for node {}: {:#}",
                        node_id,
                        e
                    );
                    if connectivity_timeout {
                        tracing::debug!(
                            target: "cmd",
                            event = "matter_on_off_read_backoff",
                            node_id,
                            endpoint,
                            backoff_secs = MATTER_ON_OFF_READ_BACKOFF.as_secs(),
                            "Matter on/off read failed with connectivity error; suppressing remaining reads for this target"
                        );
                        break;
                    }
                }
            }
        }

        Ok(false)
    }

    fn name(&self) -> &str {
        "Matter"
    }
}

#[async_trait]
impl LightController for MatterLightController {
    async fn turn_on(&self, room_id: &str, command: LightingCommand) -> LightControlResult<()> {
        let room_label =
            rhythm_os::controller_helpers::format_room_label(&self.hub_data.registry, room_id);
        let device_ids = self.target_device_ids(room_id)?;
        self.submit_turn_on_devices(&room_label, &device_ids, &command)?;
        Ok(())
    }

    async fn turn_off(&self, room_id: &str, _transition_ms: Option<u32>) -> LightControlResult<()> {
        let room_label =
            rhythm_os::controller_helpers::format_room_label(&self.hub_data.registry, room_id);
        let device_ids = self.target_device_ids(room_id)?;
        self.submit_turn_off_devices(&room_label, &device_ids)?;
        Ok(())
    }

    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        rhythm_os::controller_helpers::rooms_from_registry(&self.hub_data.registry)
    }

    async fn is_connected(&self) -> bool {
        self.transport.list_devices().is_ok()
    }

    async fn any_lights_on(&self, room_id: &str) -> LightControlResult<bool> {
        let device_ids = self.target_device_ids(room_id)?;
        self.any_lights_on_target(&HubDispatchTarget::Devices {
            native_ids: device_ids,
        })
        .await
    }

    fn name(&self) -> &str {
        "Matter"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    use crate::test_support::{RecordedOperation, SpyTransport};

    fn block_on<F: Future>(mut future: F) -> F::Output {
        fn raw_waker() -> RawWaker {
            RawWaker::new(
                std::ptr::null(),
                &RawWakerVTable::new(|_| raw_waker(), |_| {}, |_| {}, |_| {}),
            )
        }

        // SAFETY: The future is not moved after pinning.
        let mut future = unsafe { Pin::new_unchecked(&mut future) };
        // SAFETY: The RawWaker above never dereferences the data pointer.
        let waker = unsafe { Waker::from_raw(raw_waker()) };
        let mut cx = Context::from_waker(&waker);
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("Mock future returned Pending"),
        }
    }

    fn make_controller() -> (
        MatterLightController,
        Arc<SpyTransport>,
        Arc<Mutex<MatterDeviceRegistry>>,
    ) {
        let spy = Arc::new(SpyTransport::new());
        let registry = Arc::new(Mutex::new(MatterDeviceRegistry::with_options(true)));

        registry.lock().unwrap().upsert_room(
            "kitchen",
            "Kitchen",
            "kitchen",
            &["matter-42".to_string(), "matter-43".to_string()],
        );
        registry.lock().unwrap().set_area_lights(
            "kitchen",
            vec!["matter-42".to_string(), "matter-43".to_string()],
        );

        let (tx, _rx) = std::sync::mpsc::channel();
        let hub_data = Arc::new(crate::hub_state::MatterHubData {
            transport: std::sync::OnceLock::new(),
            capture_dir: std::sync::OnceLock::new(),
            registry: registry.clone(),
            fabric_id: "test".to_string(),
            commissioned: std::sync::Mutex::new(Vec::new()),
            next_node_id: std::sync::atomic::AtomicU64::new(100),
            device_caps: std::sync::Mutex::new(std::collections::HashMap::new()),
            device_quirks: std::sync::Mutex::new(std::collections::HashMap::new()),
            cloud_profiles: std::sync::Mutex::new(
                crate::cloud_profiles::CloudMatterProfileCatalog::default(),
            ),
            decommissioning: std::sync::Mutex::new(std::collections::HashSet::new()),
            recently_decommissioned: std::sync::Mutex::new(std::collections::HashMap::new()),
            node_proof_of_life: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            event_tx: tx,
        });

        let controller = MatterLightController::new(spy.clone(), hub_data);
        (controller, spy, registry)
    }

    fn set_kitchen_group(registry: &Arc<Mutex<MatterDeviceRegistry>>, group_id: u16) {
        registry.lock().unwrap().upsert_room(
            "kitchen",
            "Kitchen",
            &format_group_control_id(group_id),
            &["matter-42".to_string(), "matter-43".to_string()],
        );
    }

    fn profiled_color_bulb(
        node_id: u64,
        vendor_name: &str,
        product_name: &str,
    ) -> crate::transport::CommissionedDevice {
        crate::transport::CommissionedDevice {
            node_id,
            vendor_name: vendor_name.to_string(),
            product_name: product_name.to_string(),
            vendor_id: 0,
            product_id: 0,
            serial_number: None,
            light_endpoint: 1,
            color_modes: vec![
                crate::transport::MatterColorMode::Xy,
                crate::transport::MatterColorMode::ColorTemperature,
            ],
            min_kelvin: Some(1800),
            max_kelvin: Some(6500),
        }
    }

    fn write_count_for_node(operations: &[RecordedOperation], expected_node_id: u64) -> usize {
        operations
            .iter()
            .filter(|operation| match operation {
                RecordedOperation::SetOnOff { node_id, .. }
                | RecordedOperation::SetBrightness { node_id, .. }
                | RecordedOperation::SetColorTemperature { node_id, .. }
                | RecordedOperation::SetXy { node_id, .. }
                | RecordedOperation::SetHueSaturation { node_id, .. } => {
                    *node_id == expected_node_id
                }
                _ => false,
            })
            .count()
    }

    fn operations_for_node(
        operations: &[RecordedOperation],
        expected_node_id: u64,
    ) -> Vec<RecordedOperation> {
        operations
            .iter()
            .filter(|operation| match operation {
                RecordedOperation::SetOnOff { node_id, .. }
                | RecordedOperation::IdentifyLight { node_id, .. }
                | RecordedOperation::SetBrightness { node_id, .. }
                | RecordedOperation::SetColorTemperature { node_id, .. }
                | RecordedOperation::SetXy { node_id, .. }
                | RecordedOperation::SetHueSaturation { node_id, .. }
                | RecordedOperation::ReadOnOff { node_id, .. } => *node_id == expected_node_id,
                _ => false,
            })
            .cloned()
            .collect()
    }

    #[test]
    fn turn_on_breaks_read_only_backoff_for_nodes_marked_unreachable_at_boot() {
        let spy = Arc::new(SpyTransport::new());
        let registry = Arc::new(Mutex::new(MatterDeviceRegistry::with_options(true)));
        registry
            .lock()
            .unwrap()
            .upsert_room("r1", "Room", "r1", &["matter-42".to_string()]);
        registry
            .lock()
            .unwrap()
            .set_area_lights("r1", vec!["matter-42".to_string()]);

        let (tx, _rx) = std::sync::mpsc::channel();
        let hub_data = Arc::new(crate::hub_state::MatterHubData {
            transport: std::sync::OnceLock::new(),
            capture_dir: std::sync::OnceLock::new(),
            registry,
            fabric_id: "test".to_string(),
            commissioned: std::sync::Mutex::new(vec![crate::transport::MatterDeviceInfo {
                node_id: 42,
                vendor_name: "Vendor".to_string(),
                product_name: "Lamp".to_string(),
                reachable: false,
            }]),
            next_node_id: std::sync::atomic::AtomicU64::new(100),
            device_caps: std::sync::Mutex::new(std::collections::HashMap::from([(
                "matter-42".to_string(),
                LightCapabilities::defaults_for(LightType::ExtendedColor),
            )])),
            device_quirks: std::sync::Mutex::new(std::collections::HashMap::new()),
            cloud_profiles: std::sync::Mutex::new(
                crate::cloud_profiles::CloudMatterProfileCatalog::default(),
            ),
            decommissioning: std::sync::Mutex::new(std::collections::HashSet::new()),
            recently_decommissioned: std::sync::Mutex::new(std::collections::HashMap::new()),
            node_proof_of_life: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            event_tx: tx,
        });
        let controller = MatterLightController::new(spy.clone(), hub_data);

        assert!(!block_on(controller.any_lights_on("r1")).unwrap());
        assert!(
            spy.operations().is_empty(),
            "boot-unreachable node should suppress on/off reads while in read backoff"
        );

        block_on(controller.turn_on("r1", LightingCommand::new(50, 3000))).unwrap();

        assert!(spy.operations().iter().any(|operation| matches!(
            operation,
            RecordedOperation::SetColorTemperature { node_id: 42, .. }
        )));
        assert!(spy.operations().iter().any(|operation| matches!(
            operation,
            RecordedOperation::SetBrightness { node_id: 42, .. }
        )));
        assert!(
            block_on(controller.any_lights_on("r1")).unwrap(),
            "successful command should clear read backoff and refresh observed power"
        );
    }

    #[test]
    fn parse_device_id_basic() {
        assert_eq!(
            MatterLightController::parse_device_id("matter-42"),
            Some((42, 1))
        );
    }

    #[test]
    fn parse_device_id_with_endpoint() {
        assert_eq!(
            MatterLightController::parse_device_id("matter-42-2"),
            Some((42, 2))
        );
    }

    #[test]
    fn parse_device_id_invalid() {
        assert_eq!(MatterLightController::parse_device_id("hue-abc"), None);
    }

    #[test]
    fn parse_group_control_id_rejects_non_matter_groups() {
        assert_eq!(parse_group_control_id("kitchen"), None);
        assert_eq!(parse_group_control_id("matter-group-0"), None);
        assert_eq!(parse_group_control_id("matter-group-4097"), Some(4097));
    }

    #[test]
    fn turn_on_sends_brightness_and_color_temperature_commands_by_default() {
        let (controller, spy, _) = make_controller();
        let command = LightingCommand::new(80, 4000);

        block_on(controller.turn_on("kitchen", command)).unwrap();

        let operations = spy.operations();
        assert_eq!(operations.len(), 4);

        for node_id in [42, 43] {
            let node_operations = operations_for_node(&operations, node_id);
            assert!(matches!(
                node_operations[0],
                RecordedOperation::SetColorTemperature { endpoint: 1, .. }
            ));
            assert_eq!(
                node_operations[1],
                RecordedOperation::SetBrightness {
                    node_id,
                    endpoint: 1,
                    level: clusters::brightness_to_level(80),
                    transition_ms: None,
                }
            );
        }
    }

    #[test]
    fn turn_on_fans_out_to_devices_concurrently() {
        let (controller, spy, _) = make_controller();
        spy.delay_color_temperature(Duration::from_millis(40));

        block_on(controller.turn_on("kitchen", LightingCommand::new(80, 4000))).unwrap();

        assert_eq!(
            spy.max_concurrent_color_temperature(),
            2,
            "independent Matter endpoints should not serialize cold-session latency"
        );
    }

    /// Regression for #51: budget Matter-over-WiFi bulbs (Sengled W41-N15A,
    /// Shenzhen H6004) treat MoveToColorTemperature as a level-resetting
    /// state reload. If brightness is sent first and color second, the user's
    /// dim request snaps back to the bulb's last-known max — visible as
    /// "brightness slider never decreases". The fix is to send brightness
    /// last, after any color command, so it's the final value the bulb sees.
    #[test]
    fn turn_on_sends_brightness_after_color_temperature_so_color_cannot_clobber_level() {
        let (controller, spy, _) = make_controller();
        let command = LightingCommand::new(7, 2700);

        block_on(controller.turn_on("kitchen", command)).unwrap();

        let operations = spy.operations();
        for node_id in [42u64, 43u64] {
            let ct_idx = operations.iter().position(|operation| {
                matches!(
                    operation,
                    RecordedOperation::SetColorTemperature { node_id: n, .. } if *n == node_id
                )
            });
            let br_idx = operations.iter().position(|operation| {
                matches!(
                    operation,
                    RecordedOperation::SetBrightness { node_id: n, .. } if *n == node_id
                )
            });

            let ct_idx =
                ct_idx.unwrap_or_else(|| panic!("missing SetColorTemperature for node {node_id}"));
            let br_idx =
                br_idx.unwrap_or_else(|| panic!("missing SetBrightness for node {node_id}"));
            assert!(
                ct_idx < br_idx,
                "node {node_id}: SetBrightness must follow SetColorTemperature so cheap bulbs don't clobber the level (ct_idx={ct_idx}, br_idx={br_idx})"
            );
        }
    }

    #[test]
    fn turn_on_fans_out_to_all_devices() {
        let (controller, spy, _) = make_controller();
        let command = LightingCommand::new(50, 3000);

        block_on(controller.turn_on("kitchen", command)).unwrap();

        let node_ids: Vec<u64> = spy
            .operations()
            .iter()
            .filter_map(|operation| match operation {
                RecordedOperation::SetOnOff { node_id, .. }
                | RecordedOperation::IdentifyLight { node_id, .. }
                | RecordedOperation::SetBrightness { node_id, .. }
                | RecordedOperation::SetColorTemperature { node_id, .. }
                | RecordedOperation::SetHueSaturation { node_id, .. }
                | RecordedOperation::SetXy { node_id, .. }
                | RecordedOperation::ReadOnOff { node_id, .. } => Some(*node_id),
                _ => None,
            })
            .collect();
        assert!(node_ids.contains(&42));
        assert!(node_ids.contains(&43));
    }

    #[test]
    fn turn_on_room_with_matter_group_control_fans_out_without_groupcast_by_default() {
        let (controller, spy, registry) = make_controller();
        let group_id = 4097;
        set_kitchen_group(&registry, group_id);

        block_on(controller.turn_on("kitchen", LightingCommand::new(80, 4000))).unwrap();

        let operations = spy.operations();
        for node_id in [42, 43] {
            assert_eq!(
                operations_for_node(&operations, node_id),
                vec![
                    RecordedOperation::SetColorTemperature {
                        node_id,
                        endpoint: 1,
                        kelvin: 4000,
                        transition_ms: None,
                    },
                    RecordedOperation::SetBrightness {
                        node_id,
                        endpoint: 1,
                        level: clusters::brightness_to_level(80),
                        transition_ms: None,
                    },
                ]
            );
        }
    }

    #[test]
    fn turn_on_matter_room_applies_builtin_profiles_for_leedarson_and_sengled() {
        let (controller, spy, registry) = make_controller();
        set_kitchen_group(&registry, 65281);
        spy.set_probe_device(profiled_color_bulb(42, "Leedarson", "Smart RGBTW Bulb"));
        let mut sengled = profiled_color_bulb(43, "Sengled", "W41-N15A");
        sengled.vendor_id = 4448;
        sengled.product_id = 36866;
        sengled.color_modes = vec![
            crate::transport::MatterColorMode::HueSaturation,
            crate::transport::MatterColorMode::ColorTemperature,
        ];
        let expected_sengled = rhythm_os::controller_helpers::adapt_lighting_command(
            &crate::commissioning::build_device_capabilities(&sengled),
            &LightingCommand::new(50, 1809),
            ColorPreference::PreferHueSaturation,
        );
        spy.set_probe_device(sengled);

        block_on(controller.turn_on("kitchen", LightingCommand::new(50, 1809))).unwrap();

        let operations = spy.operations();
        assert_eq!(
            operations_for_node(&operations, 42),
            vec![
                RecordedOperation::SetOnOff {
                    node_id: 42,
                    endpoint: 1,
                    on: true,
                },
                RecordedOperation::SetColorTemperature {
                    node_id: 42,
                    endpoint: 1,
                    kelvin: 1809,
                    transition_ms: None,
                },
                RecordedOperation::SetBrightness {
                    node_id: 42,
                    endpoint: 1,
                    level: clusters::brightness_to_level(50),
                    transition_ms: None,
                },
            ]
        );
        assert_eq!(
            operations_for_node(&operations, 43),
            vec![
                RecordedOperation::SetOnOff {
                    node_id: 43,
                    endpoint: 1,
                    on: true,
                },
                RecordedOperation::SetHueSaturation {
                    node_id: 43,
                    endpoint: 1,
                    hue: expected_sengled.hue_saturation.unwrap().0,
                    saturation: expected_sengled.hue_saturation.unwrap().1,
                    transition_ms: None,
                },
                RecordedOperation::SetBrightness {
                    node_id: 43,
                    endpoint: 1,
                    level: clusters::brightness_to_level(50),
                    transition_ms: None,
                },
            ]
        );
    }

    #[test]
    fn turn_on_group_target_uses_authoritative_endpoint_plans() {
        let (mut controller, spy, _) = make_controller();
        controller.group_fanout_only = false;
        let group_id = 4097;

        block_on(controller.turn_on_target(
            &HubDispatchTarget::Group {
                room_id: "kitchen".to_string(),
                control_id: format_group_control_id(group_id),
            },
            LightingCommand::new(80, 4000),
        ))
        .unwrap();

        let operations = spy.operations();
        assert!(!operations.iter().any(|operation| matches!(
            operation,
            RecordedOperation::SetGroupColorTemperature { .. }
                | RecordedOperation::SetGroupBrightness { .. }
        )));
        for node_id in [42, 43] {
            assert_eq!(
                operations_for_node(&operations, node_id),
                vec![
                    RecordedOperation::SetColorTemperature {
                        node_id,
                        endpoint: 1,
                        kelvin: 4000,
                        transition_ms: None,
                    },
                    RecordedOperation::SetBrightness {
                        node_id,
                        endpoint: 1,
                        level: clusters::brightness_to_level(80),
                        transition_ms: None,
                    },
                ]
            );
        }
    }

    #[test]
    fn group_only_targets_require_endpoints_for_verifiable_delivery() {
        let (mut controller, spy, registry) = make_controller();
        controller.group_fanout_only = false;
        let group_id = 4097;
        registry.lock().unwrap().upsert_room(
            "group-only",
            "Group Only",
            &format_group_control_id(group_id),
            &[],
        );

        assert!(matches!(
            block_on(controller.turn_on("group-only", LightingCommand::new(80, 4000))),
            Err(LightControlError::CommandFailed(_))
        ));
        assert!(matches!(
            block_on(controller.turn_off("group-only", None)),
            Err(LightControlError::CommandFailed(_))
        ));
        block_on(controller.flash_target(&HubDispatchTarget::Group {
            room_id: "group-only".to_string(),
            control_id: format_group_control_id(group_id),
        }))
        .unwrap();

        assert_eq!(
            spy.operations(),
            vec![RecordedOperation::IdentifyGroup {
                group_id,
                duration_secs: MATTER_IDENTIFY_DURATION_SECS,
            }]
        );
    }

    #[test]
    fn fanout_only_group_targets_without_members_return_command_errors() {
        let (controller, spy, registry) = make_controller();
        let group_id = 4097;
        registry.lock().unwrap().upsert_room(
            "group-only",
            "Group Only",
            &format_group_control_id(group_id),
            &[],
        );
        let target = HubDispatchTarget::Group {
            room_id: "group-only".to_string(),
            control_id: format_group_control_id(group_id),
        };

        assert!(matches!(
            block_on(controller.turn_on_target(&target, LightingCommand::new(80, 4000))),
            Err(LightControlError::CommandFailed(_))
        ));
        assert!(matches!(
            block_on(controller.turn_off_target(&target, None)),
            Err(LightControlError::CommandFailed(_))
        ));
        assert!(matches!(
            block_on(controller.flash_target(&target)),
            Err(LightControlError::CommandFailed(_))
        ));
        assert!(spy.operations().is_empty());
    }

    #[test]
    fn turn_on_direct_device_target_succeeds() {
        let (controller, spy, _) = make_controller();

        block_on(controller.turn_on_target(
            &HubDispatchTarget::Devices {
                native_ids: vec!["matter-42".to_string()],
            },
            LightingCommand::new(50, 3000),
        ))
        .unwrap();

        assert!(
            spy.operations().iter().any(|operation| matches!(
                operation,
                RecordedOperation::SetBrightness { node_id: 42, .. }
            )),
            "expected direct device control to address node 42",
        );
    }

    #[test]
    fn turn_on_direct_device_id_without_registry_room_errors() {
        let (controller, _, _) = make_controller();

        let error =
            block_on(controller.turn_on("matter-42", LightingCommand::new(50, 3000))).unwrap_err();
        assert!(matches!(error, LightControlError::RoomNotFound(_)));
    }

    #[test]
    fn direct_device_targets_skip_invalid_ids_when_valid_members_remain() {
        let (controller, spy, _) = make_controller();

        block_on(controller.turn_on_target(
            &HubDispatchTarget::Devices {
                native_ids: vec!["not-matter".to_string(), "matter-42".to_string()],
            },
            LightingCommand::new(50, 3000),
        ))
        .unwrap();
        block_on(controller.turn_off_target(
            &HubDispatchTarget::Devices {
                native_ids: vec!["bad".to_string(), "matter-42".to_string()],
            },
            None,
        ))
        .unwrap();
        block_on(controller.flash_target(&HubDispatchTarget::Devices {
            native_ids: vec!["also-bad".to_string(), "matter-42".to_string()],
        }))
        .unwrap();
        spy.set_on_off_state(42, true);
        assert!(block_on(
            controller.any_lights_on_target(&HubDispatchTarget::Devices {
                native_ids: vec!["bad".to_string(), "matter-42".to_string()],
            })
        )
        .unwrap());

        let operations = spy.operations();
        assert!(operations.iter().any(|operation| matches!(
            operation,
            RecordedOperation::SetColorTemperature { node_id: 42, .. }
        )));
        assert!(operations.iter().any(|operation| matches!(
            operation,
            RecordedOperation::SetOnOff {
                node_id: 42,
                on: false,
                ..
            }
        )));
        assert!(operations.iter().any(|operation| matches!(
            operation,
            RecordedOperation::IdentifyLight { node_id: 42, .. }
        )));
        assert!(operations.iter().any(|operation| matches!(
            operation,
            RecordedOperation::ReadOnOff { node_id: 42, .. }
        )));
    }

    #[test]
    fn direct_device_targets_with_only_invalid_ids_return_command_errors() {
        let (controller, spy, _) = make_controller();
        let target = HubDispatchTarget::Devices {
            native_ids: vec!["not-matter".to_string()],
        };

        assert!(matches!(
            block_on(controller.turn_on_target(&target, LightingCommand::new(50, 3000))),
            Err(LightControlError::CommandFailed(_))
        ));
        assert!(matches!(
            block_on(controller.turn_off_target(&target, None)),
            Err(LightControlError::CommandFailed(_))
        ));
        assert!(matches!(
            block_on(controller.flash_target(&target)),
            Err(LightControlError::CommandFailed(_))
        ));
        assert!(!block_on(controller.any_lights_on_target(&target)).unwrap());
        assert!(spy.operations().is_empty());
    }

    #[test]
    fn turn_off_sends_off_to_all_devices() {
        let (controller, spy, _) = make_controller();

        block_on(controller.turn_off("kitchen", None)).unwrap();

        let operations = spy.operations();
        assert_eq!(operations.len(), 2);
        for node_id in [42, 43] {
            assert_eq!(
                operations_for_node(&operations, node_id),
                vec![RecordedOperation::SetOnOff {
                    node_id,
                    endpoint: 1,
                    on: false,
                }]
            );
        }
    }

    #[test]
    fn turn_off_room_with_matter_group_control_fans_out_without_groupcast_by_default() {
        let (controller, spy, registry) = make_controller();
        let group_id = 4097;
        set_kitchen_group(&registry, group_id);

        block_on(controller.turn_off("kitchen", None)).unwrap();

        let operations = spy.operations();
        assert_eq!(operations.len(), 2);
        for node_id in [42, 43] {
            assert_eq!(
                operations_for_node(&operations, node_id),
                vec![RecordedOperation::SetOnOff {
                    node_id,
                    endpoint: 1,
                    on: false,
                }]
            );
        }
    }

    #[test]
    fn turn_off_group_target_uses_authoritative_endpoint_plans() {
        let (mut controller, spy, _) = make_controller();
        controller.group_fanout_only = false;
        let group_id = 4097;

        block_on(controller.turn_off_target(
            &HubDispatchTarget::Group {
                room_id: "kitchen".to_string(),
                control_id: format_group_control_id(group_id),
            },
            None,
        ))
        .unwrap();

        let operations = spy.operations();
        assert_eq!(operations.len(), 2);
        for node_id in [42, 43] {
            assert_eq!(
                operations_for_node(&operations, node_id),
                vec![RecordedOperation::SetOnOff {
                    node_id,
                    endpoint: 1,
                    on: false,
                }]
            );
        }
    }

    #[test]
    fn turn_off_direct_device_target_succeeds() {
        let (controller, spy, _) = make_controller();

        block_on(controller.turn_off_target(
            &HubDispatchTarget::Devices {
                native_ids: vec!["matter-42".to_string()],
            },
            None,
        ))
        .unwrap();

        assert_eq!(
            spy.operations(),
            vec![RecordedOperation::SetOnOff {
                node_id: 42,
                endpoint: 1,
                on: false,
            }]
        );
    }

    #[test]
    fn flash_direct_device_target_uses_matter_identify() {
        let (controller, spy, _) = make_controller();

        block_on(controller.flash_target(&HubDispatchTarget::Devices {
            native_ids: vec!["matter-42".to_string()],
        }))
        .unwrap();

        assert_eq!(
            spy.operations(),
            vec![RecordedOperation::IdentifyLight {
                node_id: 42,
                endpoint: 1,
                duration_secs: MATTER_IDENTIFY_DURATION_SECS,
            }]
        );
    }

    #[test]
    fn flash_group_target_fans_out_to_matter_identify() {
        let (controller, spy, _) = make_controller();

        block_on(controller.flash_target(&HubDispatchTarget::Group {
            room_id: "kitchen".to_string(),
            control_id: "kitchen".to_string(),
        }))
        .unwrap();

        assert_eq!(
            spy.operations(),
            vec![
                RecordedOperation::IdentifyLight {
                    node_id: 42,
                    endpoint: 1,
                    duration_secs: MATTER_IDENTIFY_DURATION_SECS,
                },
                RecordedOperation::IdentifyLight {
                    node_id: 43,
                    endpoint: 1,
                    duration_secs: MATTER_IDENTIFY_DURATION_SECS,
                },
            ]
        );
    }

    #[test]
    fn flash_group_target_with_matter_group_control_fans_out_without_groupcast_by_default() {
        let (controller, spy, _) = make_controller();

        block_on(controller.flash_target(&HubDispatchTarget::Group {
            room_id: "kitchen".to_string(),
            control_id: format_group_control_id(4097),
        }))
        .unwrap();

        assert_eq!(
            spy.operations(),
            vec![
                RecordedOperation::IdentifyLight {
                    node_id: 42,
                    endpoint: 1,
                    duration_secs: MATTER_IDENTIFY_DURATION_SECS,
                },
                RecordedOperation::IdentifyLight {
                    node_id: 43,
                    endpoint: 1,
                    duration_secs: MATTER_IDENTIFY_DURATION_SECS,
                },
            ]
        );
    }

    #[test]
    fn flash_group_target_falls_back_when_groupcast_enabled_and_group_fails() {
        let (mut controller, spy, _) = make_controller();
        controller.group_fanout_only = false;
        let group_id = 4097;
        spy.fail_group_commands(group_id);

        block_on(controller.flash_target(&HubDispatchTarget::Group {
            room_id: "kitchen".to_string(),
            control_id: format_group_control_id(group_id),
        }))
        .unwrap();

        assert_eq!(
            spy.operations(),
            vec![
                RecordedOperation::IdentifyGroup {
                    group_id,
                    duration_secs: MATTER_IDENTIFY_DURATION_SECS,
                },
                RecordedOperation::IdentifyLight {
                    node_id: 42,
                    endpoint: 1,
                    duration_secs: MATTER_IDENTIFY_DURATION_SECS,
                },
                RecordedOperation::IdentifyLight {
                    node_id: 43,
                    endpoint: 1,
                    duration_secs: MATTER_IDENTIFY_DURATION_SECS,
                },
            ]
        );
    }

    #[test]
    fn any_lights_on_returns_true_when_device_on() {
        let (controller, spy, _) = make_controller();
        spy.set_on_off_state(42, true);

        let result = block_on(controller.any_lights_on("kitchen")).unwrap();
        assert!(result);
    }

    #[test]
    fn any_lights_on_returns_false_when_all_off() {
        let (controller, _, _) = make_controller();
        let result = block_on(controller.any_lights_on("kitchen")).unwrap();
        assert!(!result);
    }

    #[test]
    fn any_lights_on_direct_device_target_reads_state() {
        let (controller, spy, _) = make_controller();
        spy.set_on_off_state(42, true);

        let result = block_on(
            controller.any_lights_on_target(&HubDispatchTarget::Devices {
                native_ids: vec!["matter-42".to_string()],
            }),
        )
        .unwrap();
        assert!(result);
    }

    #[test]
    fn any_lights_on_backs_off_after_on_off_read_timeout() {
        let (controller, spy, _) = make_controller();
        spy.fail_read_node(42);

        let target = HubDispatchTarget::Devices {
            native_ids: vec!["matter-42".to_string()],
        };

        assert!(!block_on(controller.any_lights_on_target(&target)).unwrap());
        assert!(!block_on(controller.any_lights_on_target(&target)).unwrap());

        let read_count = spy
            .operations()
            .iter()
            .filter(|operation| {
                matches!(
                    operation,
                    RecordedOperation::ReadOnOff {
                        node_id: 42,
                        endpoint: 1
                    }
                )
            })
            .count();
        assert_eq!(
            read_count, 1,
            "second read should be suppressed while the failed endpoint is in backoff"
        );
    }

    #[test]
    fn proof_of_life_clears_read_backoff_before_next_read() {
        let (controller, spy, _) = make_controller();
        let target = HubDispatchTarget::Devices {
            native_ids: vec!["matter-42".to_string()],
        };
        spy.fail_read_node(42);

        assert!(!block_on(controller.any_lights_on_target(&target)).unwrap());

        spy.allow_read_node(42);
        spy.set_on_off_state(42, true);
        controller.hub_data.record_node_proof_of_life(42);

        assert!(block_on(controller.any_lights_on_target(&target)).unwrap());

        let read_count = spy
            .operations()
            .iter()
            .filter(|operation| {
                matches!(
                    operation,
                    RecordedOperation::ReadOnOff {
                        node_id: 42,
                        endpoint: 1
                    }
                )
            })
            .count();
        assert_eq!(
            read_count, 2,
            "proof of life should break the read backoff before the next read"
        );
    }

    #[test]
    fn controller_does_not_back_off_new_desired_state_after_write_failure() {
        let (controller, spy, _) = make_controller();
        let target = HubDispatchTarget::Devices {
            native_ids: vec!["matter-42".to_string()],
        };
        spy.timeout_node_commands(42);

        let first = block_on(controller.turn_on_target(&target, LightingCommand::new(50, 3000)));
        assert!(matches!(first, Err(LightControlError::CommandFailed(_))));
        assert_eq!(
            write_count_for_node(&spy.operations(), 42),
            1,
            "command timeout should suppress remaining writes for the endpoint"
        );

        spy.allow_node_commands(42);
        block_on(controller.turn_on_target(&target, LightingCommand::new(50, 3000))).unwrap();

        let operations = spy.operations();
        assert_eq!(
            write_count_for_node(&operations, 42),
            3,
            "new desired state should submit immediately after the failed plan retires"
        );
        assert!(operations.iter().any(|operation| matches!(
            operation,
            RecordedOperation::SetBrightness { node_id: 42, .. }
        )));
    }

    #[test]
    fn write_failure_does_not_delay_the_next_controller_plan() {
        let (controller, spy, _) = make_controller();
        let target = HubDispatchTarget::Devices {
            native_ids: vec!["matter-42".to_string()],
        };
        spy.timeout_node_commands(42);

        let first = block_on(controller.turn_on_target(&target, LightingCommand::new(50, 3000)));
        assert!(matches!(first, Err(LightControlError::CommandFailed(_))));
        assert_eq!(write_count_for_node(&spy.operations(), 42), 1);

        spy.allow_node_commands(42);
        block_on(controller.turn_on_target(&target, LightingCommand::new(50, 3000))).unwrap();

        assert_eq!(
            write_count_for_node(&spy.operations(), 42),
            3,
            "controller lanes serialize on completion rather than a wall-clock backoff"
        );
    }

    #[test]
    fn successful_identify_clears_read_backoff_before_group_fanout() {
        let (controller, spy, registry) = make_controller();
        let group_id = 4097;
        set_kitchen_group(&registry, group_id);
        let direct_target = HubDispatchTarget::Devices {
            native_ids: vec!["matter-42".to_string()],
        };
        spy.fail_read_node(42);

        assert!(!block_on(controller.any_lights_on_target(&direct_target)).unwrap());
        block_on(controller.flash_target(&direct_target)).unwrap();
        block_on(controller.turn_on("kitchen", LightingCommand::new(80, 4000))).unwrap();

        let operations = spy.operations();
        assert!(operations.iter().any(|operation| matches!(
            operation,
            RecordedOperation::IdentifyLight { node_id: 42, .. }
        )));
        assert!(operations.iter().any(|operation| matches!(
            operation,
            RecordedOperation::SetColorTemperature { node_id: 42, .. }
        )));
        assert!(operations.iter().any(|operation| matches!(
            operation,
            RecordedOperation::SetBrightness { node_id: 42, .. }
        )));
        assert!(operations.iter().any(|operation| matches!(
            operation,
            RecordedOperation::SetColorTemperature { node_id: 43, .. }
        )));
        assert!(operations.iter().any(|operation| matches!(
            operation,
            RecordedOperation::SetBrightness { node_id: 43, .. }
        )));
    }

    #[test]
    fn any_lights_on_stops_room_scan_after_connectivity_timeout() {
        let (controller, spy, _) = make_controller();
        spy.fail_read_node(42);

        assert!(!block_on(controller.any_lights_on("kitchen")).unwrap());

        assert_eq!(
            spy.operations(),
            vec![RecordedOperation::ReadOnOff {
                node_id: 42,
                endpoint: 1,
            }],
            "a timeout on one physically unavailable bulb should not serially block on every other Matter bulb in the room"
        );
    }

    #[test]
    fn any_lights_on_empty_returns_false() {
        let (controller, _, registry) = make_controller();
        registry
            .lock()
            .unwrap()
            .upsert_room("empty", "Empty Room", "empty", &[]);

        let result = block_on(controller.any_lights_on("empty")).unwrap();
        assert!(!result);
    }

    #[test]
    fn unknown_room_returns_error() {
        let (controller, _, _) = make_controller();
        let result = block_on(controller.turn_on("nonexistent", LightingCommand::new(50, 3000)));
        assert!(result.is_err());
    }

    #[test]
    fn name_returns_matter() {
        let (controller, _, _) = make_controller();
        assert_eq!(LightController::name(&controller), "Matter");
    }

    #[test]
    fn transition_time_is_forwarded() {
        let spy = Arc::new(SpyTransport::new());
        let registry = Arc::new(Mutex::new(MatterDeviceRegistry::with_options(true)));
        registry
            .lock()
            .unwrap()
            .upsert_room("r1", "Room", "r1", &["matter-1".to_string()]);
        registry
            .lock()
            .unwrap()
            .set_area_lights("r1", vec!["matter-1".to_string()]);

        let (tx, _rx) = std::sync::mpsc::channel();
        let hub_data = Arc::new(crate::hub_state::MatterHubData {
            transport: std::sync::OnceLock::new(),
            capture_dir: std::sync::OnceLock::new(),
            registry,
            fabric_id: "test".to_string(),
            commissioned: std::sync::Mutex::new(Vec::new()),
            next_node_id: std::sync::atomic::AtomicU64::new(100),
            device_caps: std::sync::Mutex::new(std::collections::HashMap::new()),
            device_quirks: std::sync::Mutex::new(std::collections::HashMap::new()),
            cloud_profiles: std::sync::Mutex::new(
                crate::cloud_profiles::CloudMatterProfileCatalog::default(),
            ),
            decommissioning: std::sync::Mutex::new(std::collections::HashSet::new()),
            recently_decommissioned: std::sync::Mutex::new(std::collections::HashMap::new()),
            node_proof_of_life: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            event_tx: tx,
        });

        let controller = MatterLightController::new(spy.clone(), hub_data);
        let command = LightingCommand::with_transition(50, 3000, 1200);

        block_on(controller.turn_on("r1", command)).unwrap();

        assert!(spy.operations().iter().any(|operation| matches!(
            operation,
            RecordedOperation::SetBrightness {
                transition_ms: Some(1200),
                ..
            }
        )));
    }

    #[test]
    fn failing_all_devices_returns_command_failed() {
        let (controller, spy, _) = make_controller();
        spy.fail_node(42);
        spy.fail_node(43);

        let result = block_on(controller.turn_on("kitchen", LightingCommand::new(50, 3000)));
        assert!(matches!(result, Err(LightControlError::CommandFailed(_))));
    }

    #[test]
    fn turn_on_probes_missing_caps_before_sending_color_command() {
        let (controller, spy, _) = make_controller();
        spy.set_probe_device(crate::transport::CommissionedDevice {
            node_id: 42,
            vendor_name: "Vendor".to_string(),
            product_name: "Lamp".to_string(),
            vendor_id: 1,
            product_id: 2,
            serial_number: None,
            light_endpoint: 1,
            color_modes: vec![crate::transport::MatterColorMode::ColorTemperature],
            min_kelvin: Some(2700),
            max_kelvin: Some(6500),
        });

        block_on(controller.turn_on_target(
            &HubDispatchTarget::Devices {
                native_ids: vec!["matter-42".to_string()],
            },
            LightingCommand::new(50, 3000),
        ))
        .unwrap();

        assert!(spy.operations().iter().any(|operation| matches!(
            operation,
            RecordedOperation::SetColorTemperature { node_id: 42, .. }
        )));
        assert!(!spy
            .operations()
            .iter()
            .any(|operation| matches!(operation, RecordedOperation::SetXy { node_id: 42, .. })));

        let cached = controller.hub_data.device_caps.lock().unwrap();
        let caps = cached
            .get("matter-42")
            .expect("expected probed caps to be cached");
        assert!(caps.supports_color_temp());
        assert!(!caps.supports_xy_color());
    }

    #[test]
    fn turn_on_direct_color_uses_hue_saturation_when_probe_falls_back() {
        let (controller, spy, _) = make_controller();

        block_on(controller.turn_on_target(
            &HubDispatchTarget::Devices {
                native_ids: vec!["matter-42".to_string()],
            },
            LightingCommand::from_color(
                50,
                rhythm_core::Rgb::new(255, 0, 0),
                rhythm_core::XyColor { x: 0.64, y: 0.33 },
                Some(400),
            ),
        ))
        .unwrap();

        assert!(spy.operations().iter().any(|operation| matches!(
            operation,
            RecordedOperation::SetHueSaturation {
                node_id: 42,
                endpoint: 1,
                hue: 0,
                saturation: 254,
                transition_ms: Some(400),
            }
        )));
        assert!(!spy
            .operations()
            .iter()
            .any(|operation| matches!(operation, RecordedOperation::SetXy { node_id: 42, .. })));
    }

    #[test]
    fn turn_on_quirked_device_prefers_xy() {
        let spy = Arc::new(SpyTransport::new());
        let registry = Arc::new(Mutex::new(MatterDeviceRegistry::with_options(true)));
        registry
            .lock()
            .unwrap()
            .upsert_room("r1", "Room", "r1", &["matter-42".to_string()]);
        registry
            .lock()
            .unwrap()
            .set_area_lights("r1", vec!["matter-42".to_string()]);

        let (tx, _rx) = std::sync::mpsc::channel();
        let hub_data = Arc::new(crate::hub_state::MatterHubData {
            transport: std::sync::OnceLock::new(),
            capture_dir: std::sync::OnceLock::new(),
            registry,
            fabric_id: "test".to_string(),
            commissioned: std::sync::Mutex::new(Vec::new()),
            next_node_id: std::sync::atomic::AtomicU64::new(100),
            device_caps: std::sync::Mutex::new(std::collections::HashMap::from([(
                "matter-42".to_string(),
                LightCapabilities::defaults_for(LightType::ExtendedColor),
            )])),
            device_quirks: std::sync::Mutex::new(std::collections::HashMap::from([(
                "matter-42".to_string(),
                vec![DeviceQuirk::NeedsXyNotCt],
            )])),
            cloud_profiles: std::sync::Mutex::new(
                crate::cloud_profiles::CloudMatterProfileCatalog::default(),
            ),
            decommissioning: std::sync::Mutex::new(std::collections::HashSet::new()),
            recently_decommissioned: std::sync::Mutex::new(std::collections::HashMap::new()),
            node_proof_of_life: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            event_tx: tx,
        });
        let controller = MatterLightController::new(spy.clone(), hub_data);

        block_on(controller.turn_on("r1", LightingCommand::new(50, 3000))).unwrap();

        assert!(spy
            .operations()
            .iter()
            .any(|operation| matches!(operation, RecordedOperation::SetXy { node_id: 42, .. })));
        assert!(!spy.operations().iter().any(|operation| matches!(
            operation,
            RecordedOperation::SetColorTemperature { node_id: 42, .. }
        )));
    }

    #[test]
    fn turn_on_direct_color_prefers_hue_saturation_when_supported() {
        let spy = Arc::new(SpyTransport::new());
        let registry = Arc::new(Mutex::new(MatterDeviceRegistry::with_options(true)));
        registry
            .lock()
            .unwrap()
            .upsert_room("r1", "Room", "r1", &["matter-42".to_string()]);
        registry
            .lock()
            .unwrap()
            .set_area_lights("r1", vec!["matter-42".to_string()]);

        let (tx, _rx) = std::sync::mpsc::channel();
        let hub_data = Arc::new(crate::hub_state::MatterHubData {
            transport: std::sync::OnceLock::new(),
            capture_dir: std::sync::OnceLock::new(),
            registry,
            fabric_id: "test".to_string(),
            commissioned: std::sync::Mutex::new(Vec::new()),
            next_node_id: std::sync::atomic::AtomicU64::new(100),
            device_caps: std::sync::Mutex::new(std::collections::HashMap::from([(
                "matter-42".to_string(),
                LightCapabilities {
                    color_modes: vec![
                        rhythm_devices::ColorMode::HueSaturation,
                        rhythm_devices::ColorMode::Xy,
                        rhythm_devices::ColorMode::ColorTemperature,
                    ],
                    ..LightCapabilities::defaults_for(LightType::ExtendedColor)
                },
            )])),
            device_quirks: std::sync::Mutex::new(std::collections::HashMap::from([(
                "matter-42".to_string(),
                Vec::new(),
            )])),
            cloud_profiles: std::sync::Mutex::new(
                crate::cloud_profiles::CloudMatterProfileCatalog::default(),
            ),
            decommissioning: std::sync::Mutex::new(std::collections::HashSet::new()),
            recently_decommissioned: std::sync::Mutex::new(std::collections::HashMap::new()),
            node_proof_of_life: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            event_tx: tx,
        });
        let controller = MatterLightController::new(spy.clone(), hub_data);

        let command = LightingCommand::from_color(
            50,
            rhythm_core::Rgb::new(255, 0, 0),
            rhythm_core::XyColor { x: 0.64, y: 0.33 },
            Some(400),
        );
        block_on(controller.turn_on("r1", command)).unwrap();

        assert!(spy.operations().iter().any(|operation| matches!(
            operation,
            RecordedOperation::SetHueSaturation { node_id: 42, .. }
        )));
        assert!(!spy
            .operations()
            .iter()
            .any(|operation| matches!(operation, RecordedOperation::SetXy { node_id: 42, .. })));
    }

    #[test]
    fn turn_on_quirked_device_sends_explicit_on_before_brightness() {
        let spy = Arc::new(SpyTransport::new());
        let registry = Arc::new(Mutex::new(MatterDeviceRegistry::with_options(true)));
        registry
            .lock()
            .unwrap()
            .upsert_room("r1", "Room", "r1", &["matter-42".to_string()]);
        registry
            .lock()
            .unwrap()
            .set_area_lights("r1", vec!["matter-42".to_string()]);

        let (tx, _rx) = std::sync::mpsc::channel();
        let hub_data = Arc::new(crate::hub_state::MatterHubData {
            transport: std::sync::OnceLock::new(),
            capture_dir: std::sync::OnceLock::new(),
            registry,
            fabric_id: "test".to_string(),
            commissioned: std::sync::Mutex::new(Vec::new()),
            next_node_id: std::sync::atomic::AtomicU64::new(100),
            device_caps: std::sync::Mutex::new(std::collections::HashMap::from([(
                "matter-42".to_string(),
                LightCapabilities::defaults_for(LightType::Dimmable),
            )])),
            device_quirks: std::sync::Mutex::new(std::collections::HashMap::from([(
                "matter-42".to_string(),
                vec![DeviceQuirk::NeedsExplicitOn],
            )])),
            cloud_profiles: std::sync::Mutex::new(
                crate::cloud_profiles::CloudMatterProfileCatalog::default(),
            ),
            decommissioning: std::sync::Mutex::new(std::collections::HashSet::new()),
            recently_decommissioned: std::sync::Mutex::new(std::collections::HashMap::new()),
            node_proof_of_life: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            event_tx: tx,
        });
        let controller = MatterLightController::new(spy.clone(), hub_data);

        block_on(controller.turn_on("r1", LightingCommand::new(50, 3000))).unwrap();

        assert_eq!(
            spy.operations(),
            vec![
                RecordedOperation::SetOnOff {
                    node_id: 42,
                    endpoint: 1,
                    on: true,
                },
                RecordedOperation::SetBrightness {
                    node_id: 42,
                    endpoint: 1,
                    level: clusters::brightness_to_level(50),
                    transition_ms: None,
                },
            ]
        );
    }

    #[test]
    fn partial_device_failure_returns_command_failed() {
        struct PartialFailureTransport {
            operations: Mutex<Vec<RecordedOperation>>,
        }

        impl PartialFailureTransport {
            fn operations(&self) -> Vec<RecordedOperation> {
                self.operations.lock().unwrap().clone()
            }
        }

        impl MatterTransport for PartialFailureTransport {
            fn commission_light(
                &self,
                _request: &crate::transport::MatterCommissionRequest,
            ) -> Result<crate::transport::CommissionedDevice> {
                unreachable!()
            }

            fn decommission_device(&self, _node_id: u64, _force: bool) -> Result<()> {
                Ok(())
            }

            fn list_devices(&self) -> Result<Vec<crate::transport::MatterDeviceInfo>> {
                Ok(Vec::new())
            }

            fn probe_light(&self, _node_id: u64) -> Result<crate::transport::CommissionedDevice> {
                unreachable!()
            }

            fn set_on_off(&self, node_id: u64, endpoint: u16, on: bool) -> Result<()> {
                self.operations
                    .lock()
                    .unwrap()
                    .push(RecordedOperation::SetOnOff {
                        node_id,
                        endpoint,
                        on,
                    });
                Ok(())
            }

            fn identify_light(
                &self,
                node_id: u64,
                endpoint: u16,
                duration_secs: u16,
            ) -> Result<()> {
                self.operations
                    .lock()
                    .unwrap()
                    .push(RecordedOperation::IdentifyLight {
                        node_id,
                        endpoint,
                        duration_secs,
                    });
                Ok(())
            }

            fn set_brightness(
                &self,
                node_id: u64,
                endpoint: u16,
                level: u8,
                transition_ms: Option<u32>,
            ) -> Result<()> {
                self.operations
                    .lock()
                    .unwrap()
                    .push(RecordedOperation::SetBrightness {
                        node_id,
                        endpoint,
                        level,
                        transition_ms,
                    });
                Ok(())
            }

            fn set_color_temperature(
                &self,
                node_id: u64,
                endpoint: u16,
                kelvin: u16,
                transition_ms: Option<u32>,
            ) -> Result<()> {
                self.operations
                    .lock()
                    .unwrap()
                    .push(RecordedOperation::SetColorTemperature {
                        node_id,
                        endpoint,
                        kelvin,
                        transition_ms,
                    });
                Ok(())
            }

            fn set_xy(
                &self,
                node_id: u64,
                endpoint: u16,
                x: f32,
                y: f32,
                transition_ms: Option<u32>,
            ) -> Result<()> {
                self.operations
                    .lock()
                    .unwrap()
                    .push(RecordedOperation::SetXy {
                        node_id,
                        endpoint,
                        x,
                        y,
                        transition_ms,
                    });
                anyhow::bail!("xy failed");
            }

            fn set_hue_saturation(
                &self,
                node_id: u64,
                endpoint: u16,
                hue: u8,
                saturation: u8,
                transition_ms: Option<u32>,
            ) -> Result<()> {
                self.operations
                    .lock()
                    .unwrap()
                    .push(RecordedOperation::SetHueSaturation {
                        node_id,
                        endpoint,
                        hue,
                        saturation,
                        transition_ms,
                    });
                anyhow::bail!("hue/saturation failed");
            }

            fn read_on_off(&self, node_id: u64, endpoint: u16) -> Result<bool> {
                self.operations
                    .lock()
                    .unwrap()
                    .push(RecordedOperation::ReadOnOff { node_id, endpoint });
                Ok(false)
            }
        }

        let transport = Arc::new(PartialFailureTransport {
            operations: Mutex::new(Vec::new()),
        });
        let registry = Arc::new(Mutex::new(MatterDeviceRegistry::with_options(true)));
        registry
            .lock()
            .unwrap()
            .upsert_room("r1", "Room", "r1", &["matter-42".to_string()]);
        registry
            .lock()
            .unwrap()
            .set_area_lights("r1", vec!["matter-42".to_string()]);

        let (tx, _rx) = std::sync::mpsc::channel();
        let hub_data = Arc::new(crate::hub_state::MatterHubData {
            transport: std::sync::OnceLock::new(),
            capture_dir: std::sync::OnceLock::new(),
            registry,
            fabric_id: "test".to_string(),
            commissioned: std::sync::Mutex::new(Vec::new()),
            next_node_id: std::sync::atomic::AtomicU64::new(100),
            device_caps: std::sync::Mutex::new(std::collections::HashMap::from([(
                "matter-42".to_string(),
                LightCapabilities::defaults_for(LightType::ExtendedColor),
            )])),
            device_quirks: std::sync::Mutex::new(std::collections::HashMap::from([(
                "matter-42".to_string(),
                vec![DeviceQuirk::NeedsXyNotCt],
            )])),
            cloud_profiles: std::sync::Mutex::new(
                crate::cloud_profiles::CloudMatterProfileCatalog::default(),
            ),
            decommissioning: std::sync::Mutex::new(std::collections::HashSet::new()),
            recently_decommissioned: std::sync::Mutex::new(std::collections::HashMap::new()),
            node_proof_of_life: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            event_tx: tx,
        });
        let controller = MatterLightController::new(transport.clone(), hub_data);

        let result = block_on(controller.turn_on("r1", LightingCommand::new(50, 3000)));
        assert!(matches!(result, Err(LightControlError::CommandFailed(_))));
        assert!(!transport.operations().iter().any(|operation| matches!(
            operation,
            RecordedOperation::SetBrightness { node_id: 42, .. }
        )));
        assert!(transport
            .operations()
            .iter()
            .any(|operation| matches!(operation, RecordedOperation::SetXy { node_id: 42, .. })));
    }

    #[test]
    fn turn_on_stops_sending_after_connectivity_timeout() {
        // An unreachable device whose first write times out must NOT receive
        // its remaining attribute writes — otherwise one dead group member
        // stalls the whole dispatch on serial CHIP timeouts (~25s each).
        // Regression for #169 ("matter failed to load").
        struct TimeoutTransport {
            operations: Mutex<Vec<RecordedOperation>>,
        }

        impl MatterTransport for TimeoutTransport {
            fn commission_light(
                &self,
                _request: &crate::transport::MatterCommissionRequest,
            ) -> Result<crate::transport::CommissionedDevice> {
                unreachable!()
            }

            fn decommission_device(&self, _node_id: u64, _force: bool) -> Result<()> {
                Ok(())
            }

            fn list_devices(&self) -> Result<Vec<crate::transport::MatterDeviceInfo>> {
                Ok(Vec::new())
            }

            fn probe_light(&self, _node_id: u64) -> Result<crate::transport::CommissionedDevice> {
                unreachable!()
            }

            fn set_on_off(&self, node_id: u64, endpoint: u16, on: bool) -> Result<()> {
                self.operations
                    .lock()
                    .unwrap()
                    .push(RecordedOperation::SetOnOff {
                        node_id,
                        endpoint,
                        on,
                    });
                anyhow::bail!("setting Matter on/off: native/chip_bridge.cc:540: CHIP Error 0x00000032: Timeout")
            }

            fn identify_light(
                &self,
                _node_id: u64,
                _endpoint: u16,
                _duration_secs: u16,
            ) -> Result<()> {
                Ok(())
            }

            fn set_brightness(
                &self,
                node_id: u64,
                endpoint: u16,
                level: u8,
                transition_ms: Option<u32>,
            ) -> Result<()> {
                self.operations
                    .lock()
                    .unwrap()
                    .push(RecordedOperation::SetBrightness {
                        node_id,
                        endpoint,
                        level,
                        transition_ms,
                    });
                anyhow::bail!("setting Matter brightness: native/chip_bridge.cc:540: CHIP Error 0x00000032: Timeout")
            }

            fn set_color_temperature(
                &self,
                node_id: u64,
                endpoint: u16,
                kelvin: u16,
                transition_ms: Option<u32>,
            ) -> Result<()> {
                self.operations
                    .lock()
                    .unwrap()
                    .push(RecordedOperation::SetColorTemperature {
                        node_id,
                        endpoint,
                        kelvin,
                        transition_ms,
                    });
                anyhow::bail!("setting Matter color temperature: native/chip_bridge.cc:540: CHIP Error 0x00000032: Timeout")
            }

            fn set_xy(
                &self,
                node_id: u64,
                endpoint: u16,
                x: f32,
                y: f32,
                transition_ms: Option<u32>,
            ) -> Result<()> {
                self.operations
                    .lock()
                    .unwrap()
                    .push(RecordedOperation::SetXy {
                        node_id,
                        endpoint,
                        x,
                        y,
                        transition_ms,
                    });
                anyhow::bail!(
                    "setting Matter xy: native/chip_bridge.cc:540: CHIP Error 0x00000032: Timeout"
                )
            }

            fn set_hue_saturation(
                &self,
                node_id: u64,
                endpoint: u16,
                hue: u8,
                saturation: u8,
                transition_ms: Option<u32>,
            ) -> Result<()> {
                self.operations
                    .lock()
                    .unwrap()
                    .push(RecordedOperation::SetHueSaturation {
                        node_id,
                        endpoint,
                        hue,
                        saturation,
                        transition_ms,
                    });
                anyhow::bail!("setting Matter hue/saturation: native/chip_bridge.cc:540: CHIP Error 0x00000032: Timeout")
            }

            fn read_on_off(&self, node_id: u64, endpoint: u16) -> Result<bool> {
                self.operations
                    .lock()
                    .unwrap()
                    .push(RecordedOperation::ReadOnOff { node_id, endpoint });
                Ok(false)
            }
        }

        let transport = Arc::new(TimeoutTransport {
            operations: Mutex::new(Vec::new()),
        });
        let registry = Arc::new(Mutex::new(MatterDeviceRegistry::with_options(true)));
        registry
            .lock()
            .unwrap()
            .upsert_room("r1", "Room", "r1", &["matter-42".to_string()]);
        registry
            .lock()
            .unwrap()
            .set_area_lights("r1", vec!["matter-42".to_string()]);

        let (tx, _rx) = std::sync::mpsc::channel();
        let hub_data = Arc::new(crate::hub_state::MatterHubData {
            transport: std::sync::OnceLock::new(),
            capture_dir: std::sync::OnceLock::new(),
            registry,
            fabric_id: "test".to_string(),
            commissioned: std::sync::Mutex::new(Vec::new()),
            next_node_id: std::sync::atomic::AtomicU64::new(100),
            device_caps: std::sync::Mutex::new(std::collections::HashMap::from([(
                "matter-42".to_string(),
                LightCapabilities::defaults_for(LightType::ExtendedColor),
            )])),
            device_quirks: std::sync::Mutex::new(std::collections::HashMap::from([(
                "matter-42".to_string(),
                vec![DeviceQuirk::NeedsXyNotCt],
            )])),
            cloud_profiles: std::sync::Mutex::new(
                crate::cloud_profiles::CloudMatterProfileCatalog::default(),
            ),
            decommissioning: std::sync::Mutex::new(std::collections::HashSet::new()),
            recently_decommissioned: std::sync::Mutex::new(std::collections::HashMap::new()),
            node_proof_of_life: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            event_tx: tx,
        });
        let controller = MatterLightController::new(transport.clone(), hub_data);

        let result = block_on(controller.turn_on("r1", LightingCommand::new(50, 3000)));
        assert!(matches!(result, Err(LightControlError::CommandFailed(_))));

        // Exactly one write: the first command timed out, so the fan-out must
        // skip the device's remaining attribute writes instead of timing out
        // on each in turn. Unpatched code records two (SetXy + SetBrightness).
        let writes = transport.operations.lock().unwrap().clone();
        assert_eq!(
            writes.len(),
            1,
            "expected the dispatch to stop after the first connectivity timeout, got {:?}",
            writes
        );
        assert!(
            !block_on(controller.any_lights_on("r1")).unwrap(),
            "backed-off endpoint should report no observed power"
        );
        let operations_after_read = transport.operations.lock().unwrap().clone();
        assert_eq!(
            operations_after_read.len(),
            1,
            "read after write timeout should be suppressed while endpoint is in backoff, got {:?}",
            operations_after_read
        );

        let retry = block_on(controller.turn_on("r1", LightingCommand::new(50, 3000)));
        assert!(matches!(retry, Err(LightControlError::CommandFailed(_))));
        let operations_after_retry = transport.operations.lock().unwrap().clone();
        assert_eq!(
            operations_after_retry.len(),
            2,
            "a new desired state should enter the endpoint lane without a time backoff, got {:?}",
            operations_after_retry
        );
    }
}
