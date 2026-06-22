//! Matter light controller using the typed Matter transport.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use async_trait::async_trait;
use log::{debug, info, warn};
use rhythm_core::controller::{
    HubDispatchTarget, HubLightController, LightControlError, LightControlResult, LightController,
};
use rhythm_core::lighting::LightingCommand;
use rhythm_core::room::Room;
use rhythm_devices::{ColorPreference, DeviceQuirk, LightCapabilities, LightType};

use crate::clusters;
use crate::hub_state::MatterHubData;
use crate::transport::MatterTransport;

/// Type alias for the registry (same as HA and Hue).
pub type MatterDeviceRegistry = rhythm_os::registry::HubDeviceRegistry;

const DISPATCH_INFO_MS: u128 = 250;
const DISPATCH_WARN_MS: u128 = 1000;
const MATTER_IDENTIFY_DURATION_SECS: u16 = 1;
const MATTER_GROUP_CONTROL_PREFIX: &str = "matter-group-";
const MATTER_GROUP_FANOUT_ONLY_ENV: &str = "RHYTHM_MATTER_GROUP_FANOUT_ONLY";
const MATTER_GROUP_SAFETY_FANOUT_ENV: &str = "RHYTHM_MATTER_GROUP_SAFETY_FANOUT";
const MATTER_ON_OFF_READ_BACKOFF: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug)]
struct MatterOnOffReadBackoff {
    suppress_until: Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MatterOnOffRead {
    On,
    Off,
    Suppressed,
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
}

impl MatterLightController {
    /// Create a new Matter light controller.
    pub fn new(transport: Arc<dyn MatterTransport>, hub_data: Arc<MatterHubData>) -> Self {
        Self {
            transport,
            hub_data,
            on_off_read_backoff: Mutex::new(HashMap::new()),
            group_fanout_only: Self::group_fanout_only_enabled(),
        }
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

    fn group_id_for_room(&self, room_id: &str) -> Option<u16> {
        self.hub_data
            .registry
            .lock()
            .ok()
            .and_then(|registry| registry.get_grouped_light_id(room_id))
            .and_then(|control_id| parse_group_control_id(&control_id))
    }

    fn group_fallback_target_label(target_label: &str, group_id: u16) -> String {
        format!("{target_label} fallback-from-group-{group_id}")
    }

    fn env_flag_enabled(name: &str, default: bool) -> bool {
        std::env::var(name)
            .map(|value| {
                let normalized = value.trim().to_ascii_lowercase();
                !matches!(normalized.as_str(), "0" | "false" | "off" | "no")
            })
            .unwrap_or(default)
    }

    fn group_fanout_only_enabled() -> bool {
        Self::env_flag_enabled(MATTER_GROUP_FANOUT_ONLY_ENV, true)
    }

    fn group_safety_fanout_enabled() -> bool {
        Self::env_flag_enabled(MATTER_GROUP_SAFETY_FANOUT_ENV, true)
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
                    LightCapabilities::defaults_for(LightType::ExtendedColor),
                    Vec::new(),
                )
            }
        }
    }

    fn common_metadata_for_devices(
        &self,
        device_ids: &[String],
    ) -> (LightCapabilities, Vec<DeviceQuirk>) {
        let mut capabilities = Vec::new();
        let mut quirks = Vec::new();

        for device_id in device_ids {
            let Some((node_id, _endpoint)) = Self::parse_device_id(device_id) else {
                continue;
            };
            let (caps, device_quirks) = self.device_metadata(device_id, node_id);
            capabilities.push(caps);
            quirks.extend(device_quirks);
        }

        let common = LightCapabilities::common_for(capabilities.iter())
            .unwrap_or_else(|| LightCapabilities::defaults_for(LightType::ExtendedColor));
        (common, quirks)
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

    fn maybe_throttle(throttle_ms: Option<u32>) {
        if let Some(throttle_ms) = throttle_ms.filter(|ms| *ms > 0) {
            std::thread::sleep(Duration::from_millis(throttle_ms as u64));
        }
    }

    fn clear_connectivity_backoff(&self, node_id: u64, endpoint: u16) {
        if let Ok(mut backoff) = self.on_off_read_backoff.lock() {
            backoff.remove(&(node_id, endpoint));
        }
    }

    fn mark_connectivity_failed(&self, node_id: u64, endpoint: u16) {
        if let Ok(mut backoff) = self.on_off_read_backoff.lock() {
            backoff.insert(
                (node_id, endpoint),
                MatterOnOffReadBackoff {
                    suppress_until: Instant::now() + MATTER_ON_OFF_READ_BACKOFF,
                },
            );
        }
    }

    fn connectivity_backoff_active(&self, node_id: u64, endpoint: u16) -> bool {
        let now = Instant::now();
        if let Ok(mut backoff) = self.on_off_read_backoff.lock() {
            match backoff.get(&(node_id, endpoint)).copied() {
                Some(entry) if entry.suppress_until > now => {
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
        if self.connectivity_backoff_active(node_id, endpoint) {
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
        let lower = error.to_string().to_ascii_lowercase();
        lower.contains("timeout")
            || lower.contains("timed out")
            || lower.contains("chip error 0x32")
            || lower.contains("failed to connect")
    }

    fn turn_on_devices(
        &self,
        target_label: &str,
        device_ids: &[String],
        command: LightingCommand,
    ) -> LightControlResult<()> {
        let started = Instant::now();
        let mut successful_devices = 0usize;
        let mut partial_devices = 0usize;
        let mut failed_devices = 0usize;

        for device_id in device_ids {
            let Some((node_id, endpoint)) = Self::parse_device_id(device_id) else {
                warn!(target: "cmd", "Matter: invalid device ID format: {}", device_id);
                failed_devices += 1;
                continue;
            };
            if self.connectivity_backoff_active(node_id, endpoint) {
                tracing::debug!(
                    target: "cmd",
                    event = "matter_command_backoff_skip",
                    node_id,
                    endpoint,
                    backoff_secs = MATTER_ON_OFF_READ_BACKOFF.as_secs(),
                    "Matter write skipped while endpoint is in connectivity backoff"
                );
                failed_devices += 1;
                continue;
            }

            let (caps, quirks) = self.device_metadata(device_id, node_id);
            let adapted = rhythm_os::controller_helpers::adapt_lighting_command(
                &caps,
                &command,
                Self::color_preference(&quirks),
            );
            let throttle_ms = quirks.iter().find_map(|quirk| match quirk {
                DeviceQuirk::CommandThrottleMs(ms) => Some(*ms),
                _ => None,
            });
            let needs_explicit_on = adapted.on && Self::needs_explicit_on(&quirks);

            let mut command_successes = 0usize;
            let mut command_failures = 0usize;
            let mut already_sent_on = false;
            // Once a write to this device fails with a connectivity timeout,
            // stop sending its remaining attribute commands. An unreachable
            // device times out on every command in turn (~25s each on the CHIP
            // stack), so a single dead group member would otherwise stall the
            // whole dispatch for >75s. The read path (`any_lights_on_target`)
            // already short-circuits the same way. See #169.
            let mut unreachable = false;

            if needs_explicit_on {
                if let Err(e) = self.transport.set_on_off(node_id, endpoint, true) {
                    warn!(
                        target: "cmd",
                        "Matter: explicit on command failed for node {}: {}",
                        node_id,
                        e
                    );
                    command_failures += 1;
                    unreachable |= self.note_connectivity_failure(node_id, endpoint, &e);
                } else {
                    command_successes += 1;
                    already_sent_on = true;
                }
                Self::maybe_throttle(throttle_ms);
            }

            // Color attributes are sent before brightness so that bulbs that
            // treat MoveToColorTemperature/MoveToColor as a level-resetting
            // state reload (observed on budget Matter-over-WiFi bulbs)
            // cannot clobber the user's requested brightness — see #51.
            if !unreachable {
                if let Some((hue, saturation)) = adapted.hue_saturation {
                    if let Err(e) = self.transport.set_hue_saturation(
                        node_id,
                        endpoint,
                        hue,
                        saturation,
                        adapted.transition_ms,
                    ) {
                        warn!(
                            target: "cmd",
                            "Matter: hue/saturation command failed for node {}: {}",
                            node_id,
                            e
                        );
                        command_failures += 1;
                        unreachable |= self.note_connectivity_failure(node_id, endpoint, &e);
                    } else {
                        command_successes += 1;
                    }
                    Self::maybe_throttle(throttle_ms);
                } else if let Some((x, y)) = adapted.xy {
                    if let Err(e) =
                        self.transport
                            .set_xy(node_id, endpoint, x, y, adapted.transition_ms)
                    {
                        warn!(
                            target: "cmd",
                            "Matter: xy command failed for node {}: {}",
                            node_id,
                            e
                        );
                        command_failures += 1;
                        unreachable |= self.note_connectivity_failure(node_id, endpoint, &e);
                    } else {
                        command_successes += 1;
                    }
                    Self::maybe_throttle(throttle_ms);
                } else if let Some(kelvin) = adapted.kelvin {
                    if let Err(e) = self.transport.set_color_temperature(
                        node_id,
                        endpoint,
                        kelvin,
                        adapted.transition_ms,
                    ) {
                        warn!(
                            target: "cmd",
                            "Matter: color temperature command failed for node {}: {}",
                            node_id,
                            e
                        );
                        command_failures += 1;
                        unreachable |= self.note_connectivity_failure(node_id, endpoint, &e);
                    } else {
                        command_successes += 1;
                    }
                    Self::maybe_throttle(throttle_ms);
                }
            }

            if !unreachable {
                if let Some(brightness) = adapted.brightness {
                    let level = clusters::brightness_to_level(brightness);
                    if let Err(e) = self.transport.set_brightness(
                        node_id,
                        endpoint,
                        level,
                        adapted.transition_ms,
                    ) {
                        warn!(
                            target: "cmd",
                            "Matter: brightness command failed for node {}: {}",
                            node_id,
                            e
                        );
                        command_failures += 1;
                        self.note_connectivity_failure(node_id, endpoint, &e);
                    } else {
                        command_successes += 1;
                    }
                    Self::maybe_throttle(throttle_ms);
                } else if adapted.on && !already_sent_on {
                    if let Err(e) = self.transport.set_on_off(node_id, endpoint, true) {
                        warn!(
                            target: "cmd",
                            "Matter: on command failed for node {}: {}",
                            node_id,
                            e
                        );
                        command_failures += 1;
                        self.note_connectivity_failure(node_id, endpoint, &e);
                    } else {
                        command_successes += 1;
                    }
                    Self::maybe_throttle(throttle_ms);
                }
            }

            if command_successes > 0 {
                self.clear_connectivity_backoff(node_id, endpoint);
            }

            match (command_successes, command_failures) {
                (0, _) => failed_devices += 1,
                (_, 0) => successful_devices += 1,
                _ => {
                    partial_devices += 1;
                    warn!(
                        target: "cmd",
                        "Matter turn_on partial for device {}: successes={} failures={}",
                        device_id,
                        command_successes,
                        command_failures
                    );
                }
            }
        }

        if successful_devices == 0 && partial_devices == 0 && !device_ids.is_empty() {
            return Err(LightControlError::CommandFailed(format!(
                "Matter turn_on failed for target {} ({} target devices)",
                target_label, failed_devices,
            )));
        }

        if successful_devices == 0 && partial_devices > 0 {
            return Err(LightControlError::CommandFailed(format!(
                "Matter turn_on partially failed for target {} ({} partial, {} failed)",
                target_label, partial_devices, failed_devices,
            )));
        }

        let latency_ms = started.elapsed().as_millis();
        if partial_devices > 0 || failed_devices > 0 {
            tracing::warn!(
                target: "cmd",
                event = "matter_turn_on_partial",
                target = %target_label,
                latency_ms,
                ok = successful_devices,
                partial = partial_devices,
                failed = failed_devices,
                device_count = device_ids.len(),
                "Matter turn_on partial"
            );
        } else if latency_ms >= DISPATCH_WARN_MS {
            tracing::warn!(
                target: "cmd",
                event = "matter_turn_on",
                target = %target_label,
                latency_ms,
                brightness = command.brightness,
                kelvin = command.kelvin,
                direct_color = command.is_direct_color,
                device_count = device_ids.len(),
                "Matter turn_on slow"
            );
        } else if latency_ms >= DISPATCH_INFO_MS {
            tracing::info!(
                target: "cmd",
                event = "matter_turn_on",
                target = %target_label,
                latency_ms,
                brightness = command.brightness,
                kelvin = command.kelvin,
                direct_color = command.is_direct_color,
                device_count = device_ids.len(),
                "Matter turn_on"
            );
        } else {
            debug!(
                target: "cmd",
                "Matter turn_on: target={} bri={} kelvin={} devices={} latency_ms={}",
                target_label,
                command.brightness,
                command.kelvin,
                device_ids.len(),
                latency_ms
            );
        }

        Ok(())
    }

    fn turn_on_group(
        &self,
        target_label: &str,
        group_id: u16,
        device_ids: &[String],
        command: LightingCommand,
    ) -> LightControlResult<()> {
        if self.group_fanout_only {
            if device_ids.is_empty() {
                return Err(LightControlError::CommandFailed(format!(
                    "Matter group turn_on fan-out-only mode has no members for target {} (group {})",
                    target_label, group_id,
                )));
            }

            let fallback_target_label = Self::group_fallback_target_label(target_label, group_id);
            Self::log_group_fanout_only(
                "turn_on",
                target_label,
                &fallback_target_label,
                group_id,
                device_ids.len(),
            );
            return self.turn_on_devices(&fallback_target_label, device_ids, command);
        }

        let started = Instant::now();
        let (caps, quirks) = self.common_metadata_for_devices(device_ids);
        let adapted = rhythm_os::controller_helpers::adapt_lighting_command(
            &caps,
            &command,
            Self::color_preference(&quirks),
        );
        let needs_explicit_on = adapted.on && Self::needs_explicit_on(&quirks);
        let mut planned_commands = Vec::new();
        if needs_explicit_on {
            planned_commands.push("OnOff.On");
        }
        if adapted.hue_saturation.is_some() {
            planned_commands.push("ColorControl.MoveToHueAndSaturation");
        } else if adapted.xy.is_some() {
            planned_commands.push("ColorControl.MoveToColor");
        } else if adapted.kelvin.is_some() {
            planned_commands.push("ColorControl.MoveToColorTemperature");
        }
        if adapted.brightness.is_some() {
            planned_commands.push("LevelControl.MoveToLevelWithOnOff");
        } else if adapted.on && !needs_explicit_on {
            planned_commands.push("OnOff.On");
        }
        let planned_commands = planned_commands.join(",");
        let mut command_successes = 0usize;
        let mut command_failures = 0usize;
        let mut already_sent_on = false;

        info!(
            target: "cmd",
            "Matter group turn_on dispatch: target={} group_id={} members={} commands={} brightness={} kelvin={} direct_color={} transition_ms={:?}",
            target_label,
            group_id,
            device_ids.len(),
            planned_commands,
            command.brightness,
            command.kelvin,
            command.is_direct_color,
            command.transition_ms
        );

        if needs_explicit_on {
            if let Err(e) = self.transport.set_group_on_off(group_id, true) {
                warn!(
                    target: "cmd",
                    "Matter: explicit group on command failed for group {}: {}",
                    group_id,
                    e
                );
                command_failures += 1;
            } else {
                command_successes += 1;
                already_sent_on = true;
            }
        }

        if let Some((hue, saturation)) = adapted.hue_saturation {
            if let Err(e) = self.transport.set_group_hue_saturation(
                group_id,
                hue,
                saturation,
                adapted.transition_ms,
            ) {
                warn!(
                    target: "cmd",
                    "Matter: group hue/saturation command failed for group {}: {}",
                    group_id,
                    e
                );
                command_failures += 1;
            } else {
                command_successes += 1;
            }
        } else if let Some((x, y)) = adapted.xy {
            if let Err(e) = self
                .transport
                .set_group_xy(group_id, x, y, adapted.transition_ms)
            {
                warn!(
                    target: "cmd",
                    "Matter: group xy command failed for group {}: {}",
                    group_id,
                    e
                );
                command_failures += 1;
            } else {
                command_successes += 1;
            }
        } else if let Some(kelvin) = adapted.kelvin {
            if let Err(e) =
                self.transport
                    .set_group_color_temperature(group_id, kelvin, adapted.transition_ms)
            {
                warn!(
                    target: "cmd",
                    "Matter: group color temperature command failed for group {}: {}",
                    group_id,
                    e
                );
                command_failures += 1;
            } else {
                command_successes += 1;
            }
        }

        if let Some(brightness) = adapted.brightness {
            let level = clusters::brightness_to_level(brightness);
            if let Err(e) =
                self.transport
                    .set_group_brightness(group_id, level, adapted.transition_ms)
            {
                warn!(
                    target: "cmd",
                    "Matter: group brightness command failed for group {}: {}",
                    group_id,
                    e
                );
                command_failures += 1;
            } else {
                command_successes += 1;
            }
        } else if adapted.on && !already_sent_on {
            if let Err(e) = self.transport.set_group_on_off(group_id, true) {
                warn!(
                    target: "cmd",
                    "Matter: group on command failed for group {}: {}",
                    group_id,
                    e
                );
                command_failures += 1;
            } else {
                command_successes += 1;
            }
        }

        if command_successes == 0 {
            if device_ids.is_empty() {
                return Err(LightControlError::CommandFailed(format!(
                    "Matter group turn_on failed for target {} (group {}, {} failures, no fallback members)",
                    target_label, group_id, command_failures,
                )));
            }

            let fallback_target_label = Self::group_fallback_target_label(target_label, group_id);
            tracing::warn!(
                target: "cmd",
                event = "matter_group_turn_on_fallback",
                target = %target_label,
                fallback_target = %fallback_target_label,
                group_id,
                failed = command_failures,
                member_count = device_ids.len(),
                "Matter group turn_on failed; falling back to member fan-out"
            );
            return self.turn_on_devices(&fallback_target_label, device_ids, command);
        }

        let latency_ms = started.elapsed().as_millis();
        if command_failures > 0 {
            tracing::warn!(
                target: "cmd",
                event = "matter_group_turn_on_partial",
                target = %target_label,
                group_id,
                latency_ms,
                ok = command_successes,
                failed = command_failures,
                member_count = device_ids.len(),
                "Matter group turn_on partial"
            );
        } else if latency_ms >= DISPATCH_WARN_MS {
            tracing::warn!(
                target: "cmd",
                event = "matter_group_turn_on",
                target = %target_label,
                group_id,
                latency_ms,
                brightness = command.brightness,
                kelvin = command.kelvin,
                direct_color = command.is_direct_color,
                member_count = device_ids.len(),
                "Matter group turn_on slow"
            );
        } else if latency_ms >= DISPATCH_INFO_MS {
            tracing::info!(
                target: "cmd",
                event = "matter_group_turn_on",
                target = %target_label,
                group_id,
                latency_ms,
                brightness = command.brightness,
                kelvin = command.kelvin,
                direct_color = command.is_direct_color,
                member_count = device_ids.len(),
                "Matter group turn_on"
            );
        } else {
            info!(
                target: "cmd",
                "Matter group turn_on sent: target={} group_id={} members={} commands={} brightness={} kelvin={} latency_ms={}",
                target_label,
                group_id,
                device_ids.len(),
                planned_commands,
                command.brightness,
                command.kelvin,
                latency_ms
            );
        }

        if Self::group_safety_fanout_enabled() && !device_ids.is_empty() {
            let fallback_target_label = Self::group_fallback_target_label(target_label, group_id);
            Self::log_group_safety_fanout(
                "turn_on",
                target_label,
                &fallback_target_label,
                group_id,
                device_ids.len(),
                command_successes,
                command_failures,
            );
            return self.turn_on_devices(&fallback_target_label, device_ids, command);
        }

        Ok(())
    }

    fn turn_off_devices(
        &self,
        target_label: &str,
        device_ids: &[String],
    ) -> LightControlResult<()> {
        let started = Instant::now();
        let mut successful_devices = 0usize;
        let mut failed_devices = 0usize;

        for device_id in device_ids {
            let Some((node_id, endpoint)) = Self::parse_device_id(device_id) else {
                failed_devices += 1;
                continue;
            };
            if self.connectivity_backoff_active(node_id, endpoint) {
                tracing::debug!(
                    target: "cmd",
                    event = "matter_command_backoff_skip",
                    node_id,
                    endpoint,
                    backoff_secs = MATTER_ON_OFF_READ_BACKOFF.as_secs(),
                    "Matter off skipped while endpoint is in connectivity backoff"
                );
                failed_devices += 1;
                continue;
            }

            if let Err(e) = self.transport.set_on_off(node_id, endpoint, false) {
                warn!(target: "cmd", "Matter: off command failed for node {}: {}", node_id, e);
                self.note_connectivity_failure(node_id, endpoint, &e);
                failed_devices += 1;
            } else {
                successful_devices += 1;
                self.clear_connectivity_backoff(node_id, endpoint);
            }
        }

        if successful_devices == 0 && !device_ids.is_empty() {
            return Err(LightControlError::CommandFailed(format!(
                "Matter turn_off failed for target {} ({} target devices)",
                target_label, failed_devices,
            )));
        }

        let latency_ms = started.elapsed().as_millis();
        if failed_devices > 0 {
            tracing::warn!(
                target: "cmd",
                event = "matter_turn_off_partial",
                target = %target_label,
                latency_ms,
                ok = successful_devices,
                failed = failed_devices,
                device_count = device_ids.len(),
                "Matter turn_off partial"
            );
        } else if latency_ms >= DISPATCH_WARN_MS {
            tracing::warn!(
                target: "cmd",
                event = "matter_turn_off",
                target = %target_label,
                latency_ms,
                device_count = device_ids.len(),
                "Matter turn_off slow"
            );
        } else if latency_ms >= DISPATCH_INFO_MS {
            tracing::info!(
                target: "cmd",
                event = "matter_turn_off",
                target = %target_label,
                latency_ms,
                device_count = device_ids.len(),
                "Matter turn_off"
            );
        } else {
            debug!(
                target: "cmd",
                "Matter turn_off: target={} devices={} latency_ms={}",
                target_label,
                device_ids.len(),
                latency_ms
            );
        }

        Ok(())
    }

    fn turn_off_group(
        &self,
        target_label: &str,
        group_id: u16,
        device_ids: &[String],
    ) -> LightControlResult<()> {
        if self.group_fanout_only {
            if device_ids.is_empty() {
                return Err(LightControlError::CommandFailed(format!(
                    "Matter group turn_off fan-out-only mode has no members for target {} (group {})",
                    target_label, group_id,
                )));
            }

            let fallback_target_label = Self::group_fallback_target_label(target_label, group_id);
            Self::log_group_fanout_only(
                "turn_off",
                target_label,
                &fallback_target_label,
                group_id,
                device_ids.len(),
            );
            return self.turn_off_devices(&fallback_target_label, device_ids);
        }

        let started = Instant::now();
        info!(
            target: "cmd",
            "Matter group turn_off dispatch: target={} group_id={} command=OnOff.Off",
            target_label,
            group_id
        );
        if let Err(e) = self.transport.set_group_on_off(group_id, false) {
            if device_ids.is_empty() {
                return Err(LightControlError::CommandFailed(format!(
                    "Matter group turn_off failed for target {} (group {}, no fallback members): {}",
                    target_label, group_id, e
                )));
            }

            let fallback_target_label = Self::group_fallback_target_label(target_label, group_id);
            tracing::warn!(
                target: "cmd",
                event = "matter_group_turn_off_fallback",
                target = %target_label,
                fallback_target = %fallback_target_label,
                group_id,
                member_count = device_ids.len(),
                error = %e,
                "Matter group turn_off failed; falling back to member fan-out"
            );
            return self.turn_off_devices(&fallback_target_label, device_ids);
        }

        let latency_ms = started.elapsed().as_millis();
        if latency_ms >= DISPATCH_WARN_MS {
            tracing::warn!(
                target: "cmd",
                event = "matter_group_turn_off",
                target = %target_label,
                group_id,
                latency_ms,
                "Matter group turn_off slow"
            );
        } else if latency_ms >= DISPATCH_INFO_MS {
            tracing::info!(
                target: "cmd",
                event = "matter_group_turn_off",
                target = %target_label,
                group_id,
                latency_ms,
                "Matter group turn_off"
            );
        } else {
            info!(
                target: "cmd",
                "Matter group turn_off sent: target={} group_id={} latency_ms={}",
                target_label, group_id, latency_ms
            );
        }

        if Self::group_safety_fanout_enabled() && !device_ids.is_empty() {
            let fallback_target_label = Self::group_fallback_target_label(target_label, group_id);
            Self::log_group_safety_fanout(
                "turn_off",
                target_label,
                &fallback_target_label,
                group_id,
                device_ids.len(),
                1,
                0,
            );
            return self.turn_off_devices(&fallback_target_label, device_ids);
        }

        Ok(())
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
            } else {
                successful_devices += 1;
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
        let target_label = target.label();
        let device_ids = self.target_device_ids_for_target(target)?;
        if let Some(group_id) = Self::group_id_for_target(target) {
            return self.turn_on_group(&target_label, group_id, &device_ids, command);
        }
        self.turn_on_devices(&target_label, &device_ids, command)
    }

    async fn turn_off_target(
        &self,
        target: &HubDispatchTarget,
        _transition_ms: Option<u32>,
    ) -> LightControlResult<()> {
        let target_label = target.label();
        let device_ids = self.target_device_ids_for_target(target)?;
        if let Some(group_id) = Self::group_id_for_target(target) {
            return self.turn_off_group(&target_label, group_id, &device_ids);
        }
        self.turn_off_devices(&target_label, &device_ids)
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
                    warn!(
                        target: "cmd",
                        "Matter: failed to read on/off state for node {}: {}",
                        node_id,
                        e
                    );
                    if Self::looks_like_connectivity_timeout(&e) {
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
        if let Some(group_id) = self.group_id_for_room(room_id) {
            return self.turn_on_group(&room_label, group_id, &device_ids, command);
        }
        self.turn_on_devices(&room_label, &device_ids, command)
    }

    async fn turn_off(&self, room_id: &str, _transition_ms: Option<u32>) -> LightControlResult<()> {
        let room_label =
            rhythm_os::controller_helpers::format_room_label(&self.hub_data.registry, room_id);
        let device_ids = self.target_device_ids(room_id)?;
        if let Some(group_id) = self.group_id_for_room(room_id) {
            return self.turn_off_group(&room_label, group_id, &device_ids);
        }
        self.turn_off_devices(&room_label, &device_ids)
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

        assert!(matches!(
            operations[0],
            RecordedOperation::SetColorTemperature {
                node_id: 42,
                endpoint: 1,
                ..
            }
        ));
        assert_eq!(
            operations[1],
            RecordedOperation::SetBrightness {
                node_id: 42,
                endpoint: 1,
                level: clusters::brightness_to_level(80),
                transition_ms: None,
            }
        );
        assert!(matches!(
            operations[2],
            RecordedOperation::SetColorTemperature {
                node_id: 43,
                endpoint: 1,
                ..
            }
        ));
        assert_eq!(
            operations[3],
            RecordedOperation::SetBrightness {
                node_id: 43,
                endpoint: 1,
                level: clusters::brightness_to_level(80),
                transition_ms: None,
            }
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

        assert_eq!(
            spy.operations(),
            vec![
                RecordedOperation::SetColorTemperature {
                    node_id: 42,
                    endpoint: 1,
                    kelvin: 4000,
                    transition_ms: None,
                },
                RecordedOperation::SetBrightness {
                    node_id: 42,
                    endpoint: 1,
                    level: clusters::brightness_to_level(80),
                    transition_ms: None,
                },
                RecordedOperation::SetColorTemperature {
                    node_id: 43,
                    endpoint: 1,
                    kelvin: 4000,
                    transition_ms: None,
                },
                RecordedOperation::SetBrightness {
                    node_id: 43,
                    endpoint: 1,
                    level: clusters::brightness_to_level(80),
                    transition_ms: None,
                },
            ]
        );
    }

    #[test]
    fn turn_on_group_target_falls_back_when_groupcast_enabled_and_group_fails() {
        let (mut controller, spy, _) = make_controller();
        controller.group_fanout_only = false;
        let group_id = 4097;
        spy.fail_group_commands(group_id);

        block_on(controller.turn_on_target(
            &HubDispatchTarget::Group {
                room_id: "kitchen".to_string(),
                control_id: format_group_control_id(group_id),
            },
            LightingCommand::new(80, 4000),
        ))
        .unwrap();

        assert_eq!(
            spy.operations(),
            vec![
                RecordedOperation::SetGroupColorTemperature {
                    group_id,
                    kelvin: 4000,
                    transition_ms: None,
                },
                RecordedOperation::SetGroupBrightness {
                    group_id,
                    level: clusters::brightness_to_level(80),
                    transition_ms: None,
                },
                RecordedOperation::SetColorTemperature {
                    node_id: 42,
                    endpoint: 1,
                    kelvin: 4000,
                    transition_ms: None,
                },
                RecordedOperation::SetBrightness {
                    node_id: 42,
                    endpoint: 1,
                    level: clusters::brightness_to_level(80),
                    transition_ms: None,
                },
                RecordedOperation::SetColorTemperature {
                    node_id: 43,
                    endpoint: 1,
                    kelvin: 4000,
                    transition_ms: None,
                },
                RecordedOperation::SetBrightness {
                    node_id: 43,
                    endpoint: 1,
                    level: clusters::brightness_to_level(80),
                    transition_ms: None,
                },
            ]
        );
    }

    #[test]
    fn group_only_targets_can_use_groupcast_when_fanout_only_is_disabled() {
        let (mut controller, spy, registry) = make_controller();
        controller.group_fanout_only = false;
        let group_id = 4097;
        registry.lock().unwrap().upsert_room(
            "group-only",
            "Group Only",
            &format_group_control_id(group_id),
            &[],
        );

        block_on(controller.turn_on("group-only", LightingCommand::new(80, 4000))).unwrap();
        block_on(controller.turn_off("group-only", None)).unwrap();
        block_on(controller.flash_target(&HubDispatchTarget::Group {
            room_id: "group-only".to_string(),
            control_id: format_group_control_id(group_id),
        }))
        .unwrap();

        assert_eq!(
            spy.operations(),
            vec![
                RecordedOperation::SetGroupColorTemperature {
                    group_id,
                    kelvin: 4000,
                    transition_ms: None,
                },
                RecordedOperation::SetGroupBrightness {
                    group_id,
                    level: clusters::brightness_to_level(80),
                    transition_ms: None,
                },
                RecordedOperation::SetGroupOnOff {
                    group_id,
                    on: false,
                },
                RecordedOperation::IdentifyGroup {
                    group_id,
                    duration_secs: MATTER_IDENTIFY_DURATION_SECS,
                },
            ]
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

        assert_eq!(
            spy.operations(),
            vec![
                RecordedOperation::SetOnOff {
                    node_id: 42,
                    endpoint: 1,
                    on: false,
                },
                RecordedOperation::SetOnOff {
                    node_id: 43,
                    endpoint: 1,
                    on: false,
                },
            ]
        );
    }

    #[test]
    fn turn_off_room_with_matter_group_control_fans_out_without_groupcast_by_default() {
        let (controller, spy, registry) = make_controller();
        let group_id = 4097;
        set_kitchen_group(&registry, group_id);

        block_on(controller.turn_off("kitchen", None)).unwrap();

        assert_eq!(
            spy.operations(),
            vec![
                RecordedOperation::SetOnOff {
                    node_id: 42,
                    endpoint: 1,
                    on: false,
                },
                RecordedOperation::SetOnOff {
                    node_id: 43,
                    endpoint: 1,
                    on: false,
                },
            ]
        );
    }

    #[test]
    fn turn_off_group_target_falls_back_when_groupcast_enabled_and_group_fails() {
        let (mut controller, spy, _) = make_controller();
        controller.group_fanout_only = false;
        let group_id = 4097;
        spy.fail_group_commands(group_id);

        block_on(controller.turn_off_target(
            &HubDispatchTarget::Group {
                room_id: "kitchen".to_string(),
                control_id: format_group_control_id(group_id),
            },
            None,
        ))
        .unwrap();

        assert_eq!(
            spy.operations(),
            vec![
                RecordedOperation::SetGroupOnOff {
                    group_id,
                    on: false,
                },
                RecordedOperation::SetOnOff {
                    node_id: 42,
                    endpoint: 1,
                    on: false,
                },
                RecordedOperation::SetOnOff {
                    node_id: 43,
                    endpoint: 1,
                    on: false,
                },
            ]
        );
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
            event_tx: tx,
        });
        let controller = MatterLightController::new(transport.clone(), hub_data);

        let result = block_on(controller.turn_on("r1", LightingCommand::new(50, 3000)));
        assert!(matches!(result, Err(LightControlError::CommandFailed(_))));
        assert!(transport.operations().iter().any(|operation| matches!(
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
            1,
            "write retry should be skipped while endpoint is in backoff, got {:?}",
            operations_after_retry
        );
    }
}
