//! Matter bulb tester commands and report persistence.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(not(test))]
use std::thread;
#[cfg(not(test))]
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::HubType;
use rhythm_os::state::SharedState;
use serde_json::{json, Map, Value};

use crate::hub_state::MatterHubData;
use crate::transport::{
    CommissionedDevice, MatterColorMode, MatterLevelCommandVariant, MatterLevelStepMode,
    MatterTransport,
};

const DEFAULT_IDENTIFY_SECS: u16 = 2;
const DEFAULT_BRIGHTNESS_LEVEL: u8 = 128;
const VISUAL_BASELINE_PERCENT: u8 = 70;
const VISUAL_BASELINE_KELVIN: u16 = 4000;
const DEFAULT_WARM_KELVIN: u16 = 2700;
const DEFAULT_COOL_KELVIN: u16 = 6500;
const MIN_REASONABLE_CT_KELVIN: u16 = 1500;
const MAX_REASONABLE_CT_KELVIN: u16 = 10000;
const LOW_DIM_PERCENT: u8 = 3;
const DIM_RAMP_START_PERCENT: u8 = 85;
const DIM_RAMP_END_PERCENT: u8 = 10;
const DIM_RAMP_TRANSITION_MS: u32 = 3000;
const ON_LEVEL_RESTORE_PERCENT: u8 = 10;
const POWER_CYCLE_SETUP_PERCENT: u8 = 50;
const XY_RED: (f32, f32) = (0.70, 0.30);
const XY_GREEN: (f32, f32) = (0.17, 0.70);
const XY_BLUE: (f32, f32) = (0.15, 0.06);
const XY_NEUTRAL_WHITE: (f32, f32) = (0.3127, 0.3290);
const HS_RED: (u8, u8) = (0, 254);
const HS_GREEN: (u8, u8) = (85, 254);
const HS_BLUE: (u8, u8) = (170, 254);
const HS_NEUTRAL_WHITE: (u8, u8) = (0, 0);
const RAPID_THRESHOLDS_MS: [u64; 5] = [50, 100, 200, 500, 1000];

pub fn run_bulb_test(state: &SharedState, params: &Value) -> Result<Value> {
    let device_id = params
        .get("device_id")
        .and_then(Value::as_str)
        .context("Missing device_id")?;
    let test = params
        .get("test")
        .and_then(Value::as_str)
        .context("Missing test")?;

    let native_id = resolve_matter_device_id(state, device_id)?;
    let (node_id, endpoint) = crate::lifecycle::parse_device_id(&native_id)
        .with_context(|| format!("Invalid Matter device ID: {}", native_id))?;
    let hub_data = get_hub_data(state)?;
    let transport = hub_data
        .transport
        .get()
        .cloned()
        .context("Matter transport not initialized")?;

    let (claimed_device, probe_error) = match transport.probe_light(node_id) {
        Ok(device) => (Some(device), None),
        Err(error) => (None, Some(error.to_string())),
    };
    let raw_capability_snapshot = transport
        .read_light_capability_snapshot(node_id, endpoint)
        .ok();
    let claimed_capabilities = claimed_capabilities(
        node_id,
        endpoint,
        claimed_device.as_ref(),
        probe_error.as_deref(),
        raw_capability_snapshot.as_ref(),
    );
    let endpoint_validation = endpoint_validation(endpoint, claimed_device.as_ref());
    let (warm_kelvin, cool_kelvin, color_temperature_range_source) =
        color_temperature_targets(claimed_device.as_ref());

    let mut commands = Vec::new();
    match test {
        "identify" => {
            run_light_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "identify",
                "Identify.Identify",
                json!({"duration_secs": DEFAULT_IDENTIFY_SECS}),
                false,
                || transport.identify_light(node_id, endpoint, DEFAULT_IDENTIFY_SECS),
            );
        }
        "turn_off" => {
            run_light_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "set_on_off_false",
                "OnOff.Off",
                json!({"on": false}),
                true,
                || transport.set_on_off(node_id, endpoint, false),
            );
        }
        "brightness_without_on" => {
            run_light_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "set_on_off_false",
                "OnOff.Off",
                json!({"on": false}),
                false,
                || transport.set_on_off(node_id, endpoint, false),
            );
            sleep_ms(250);
            run_brightness_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "set_brightness_without_on",
                DEFAULT_BRIGHTNESS_LEVEL,
                None,
                true,
            );
        }
        "brightness_with_on" => {
            run_light_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "set_on_off_false",
                "OnOff.Off",
                json!({"on": false}),
                false,
                || transport.set_on_off(node_id, endpoint, false),
            );
            sleep_ms(250);
            run_light_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "set_on_off_true",
                "OnOff.On",
                json!({"on": true}),
                false,
                || transport.set_on_off(node_id, endpoint, true),
            );
            sleep_ms(100);
            run_brightness_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "set_brightness_after_on",
                DEFAULT_BRIGHTNESS_LEVEL,
                None,
                true,
            );
        }
        "level_move_to_level" => {
            run_light_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "set_on_off_false",
                "OnOff.Off",
                json!({"on": false}),
                false,
                || transport.set_on_off(node_id, endpoint, false),
            );
            sleep_ms(250);
            run_level_variant_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "level_move_to_level",
                MatterLevelCommandVariant::MoveToLevel,
                DEFAULT_BRIGHTNESS_LEVEL,
                None,
                None,
                true,
            );
        }
        "level_move_to_level_with_onoff" => {
            run_light_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "set_on_off_false",
                "OnOff.Off",
                json!({"on": false}),
                false,
                || transport.set_on_off(node_id, endpoint, false),
            );
            sleep_ms(250);
            run_level_variant_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "level_move_to_level_with_onoff",
                MatterLevelCommandVariant::MoveToLevelWithOnOff,
                DEFAULT_BRIGHTNESS_LEVEL,
                None,
                None,
                true,
            );
        }
        "level_step" => {
            set_visual_baseline(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                claimed_device.as_ref(),
            );
            run_level_variant_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "level_step_down",
                MatterLevelCommandVariant::Step,
                80,
                Some(MatterLevelStepMode::Down),
                Some(600),
                true,
            );
        }
        "level_step_with_onoff" => {
            run_light_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "set_on_off_false",
                "OnOff.Off",
                json!({"on": false}),
                false,
                || transport.set_on_off(node_id, endpoint, false),
            );
            sleep_ms(250);
            run_level_variant_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "level_step_with_onoff_up",
                MatterLevelCommandVariant::StepWithOnOff,
                80,
                Some(MatterLevelStepMode::Up),
                Some(600),
                true,
            );
        }
        "dim_low" => {
            set_visual_baseline(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                claimed_device.as_ref(),
            );
            run_brightness_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "set_brightness_low_dim",
                crate::clusters::brightness_to_level(LOW_DIM_PERCENT),
                None,
                true,
            );
        }
        "dim_ramp" => {
            set_visual_baseline(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                claimed_device.as_ref(),
            );
            run_brightness_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "set_brightness_ramp_start",
                crate::clusters::brightness_to_level(DIM_RAMP_START_PERCENT),
                None,
                false,
            );
            sleep_ms(400);
            run_brightness_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "set_brightness_ramp_down",
                crate::clusters::brightness_to_level(DIM_RAMP_END_PERCENT),
                Some(DIM_RAMP_TRANSITION_MS),
                true,
            );
        }
        "brightness_steps" => {
            set_visual_baseline(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                claimed_device.as_ref(),
            );
            for brightness in [20, 60, 100, 40] {
                run_brightness_command(
                    &mut commands,
                    &transport,
                    node_id,
                    endpoint,
                    &format!("set_brightness_{}", brightness),
                    crate::clusters::brightness_to_level(brightness),
                    None,
                    false,
                );
                sleep_ms(450);
            }
            read_on_off_command(&mut commands, &transport, node_id, endpoint);
        }
        "color_temperature" | "color_temperature_warm" => {
            set_visual_baseline(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                claimed_device.as_ref(),
            );
            run_color_temperature_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "set_color_temperature_warm",
                warm_kelvin,
                None,
                true,
            );
        }
        "color_temperature_cool" => {
            set_visual_baseline(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                claimed_device.as_ref(),
            );
            run_color_temperature_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "set_color_temperature_cool",
                cool_kelvin,
                None,
                true,
            );
        }
        "xy_color" | "xy_red" => {
            set_visual_baseline(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                claimed_device.as_ref(),
            );
            run_xy_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "set_xy_red",
                XY_RED,
                true,
            );
        }
        "xy_green" => {
            set_visual_baseline(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                claimed_device.as_ref(),
            );
            run_xy_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "set_xy_green",
                XY_GREEN,
                true,
            );
        }
        "xy_blue" => {
            set_visual_baseline(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                claimed_device.as_ref(),
            );
            run_xy_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "set_xy_blue",
                XY_BLUE,
                true,
            );
        }
        "hue_sat_red" => {
            set_visual_baseline(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                claimed_device.as_ref(),
            );
            run_hue_saturation_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "set_hue_sat_red",
                HS_RED,
                true,
            );
        }
        "hue_sat_green" => {
            set_visual_baseline(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                claimed_device.as_ref(),
            );
            run_hue_saturation_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "set_hue_sat_green",
                HS_GREEN,
                true,
            );
        }
        "hue_sat_blue" => {
            set_visual_baseline(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                claimed_device.as_ref(),
            );
            run_hue_saturation_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "set_hue_sat_blue",
                HS_BLUE,
                true,
            );
        }
        "ct_to_xy" => {
            set_visual_baseline(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                claimed_device.as_ref(),
            );
            run_color_temperature_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "switch_start_color_temperature",
                warm_kelvin,
                None,
                false,
            );
            sleep_ms(500);
            run_xy_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "switch_ct_to_xy_red",
                XY_RED,
                true,
            );
        }
        "xy_to_ct" => {
            set_visual_baseline(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                claimed_device.as_ref(),
            );
            run_xy_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "switch_start_xy_blue",
                XY_BLUE,
                false,
            );
            sleep_ms(500);
            run_color_temperature_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "switch_xy_to_color_temperature",
                warm_kelvin,
                None,
                true,
            );
        }
        "ct_to_hue_sat" => {
            set_visual_baseline(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                claimed_device.as_ref(),
            );
            run_color_temperature_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "switch_start_color_temperature",
                warm_kelvin,
                None,
                false,
            );
            sleep_ms(500);
            run_hue_saturation_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "switch_ct_to_hue_sat_blue",
                HS_BLUE,
                true,
            );
        }
        "hue_sat_to_ct" => {
            set_visual_baseline(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                claimed_device.as_ref(),
            );
            run_hue_saturation_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "switch_start_hue_sat_blue",
                HS_BLUE,
                false,
            );
            sleep_ms(500);
            run_color_temperature_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "switch_hue_sat_to_color_temperature",
                warm_kelvin,
                None,
                true,
            );
        }
        "on_level_restore" => {
            set_visual_baseline(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                claimed_device.as_ref(),
            );
            run_brightness_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "set_restore_test_low_level",
                crate::clusters::brightness_to_level(ON_LEVEL_RESTORE_PERCENT),
                None,
                false,
            );
            sleep_ms(400);
            run_light_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "restore_test_set_off",
                "OnOff.Off",
                json!({"on": false}),
                false,
                || transport.set_on_off(node_id, endpoint, false),
            );
            sleep_ms(500);
            run_light_command(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                "restore_test_set_on",
                "OnOff.On",
                json!({"on": true}),
                true,
                || transport.set_on_off(node_id, endpoint, true),
            );
        }
        "power_on_behavior" => {
            run_power_cycle_setup(&mut commands, &transport, node_id, endpoint, warm_kelvin);
        }
        "rapid_commands" => {
            set_visual_baseline(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                claimed_device.as_ref(),
            );
            run_rapid_burst(
                &mut commands,
                &transport,
                node_id,
                endpoint,
                claimed_device.as_ref(),
                0,
                warm_kelvin,
                cool_kelvin,
            );
        }
        "read_on_off" => {
            read_on_off_command(&mut commands, &transport, node_id, endpoint);
        }
        other => {
            if let Some(gap_ms) = rapid_threshold_ms(other) {
                set_visual_baseline(
                    &mut commands,
                    &transport,
                    node_id,
                    endpoint,
                    claimed_device.as_ref(),
                );
                run_rapid_burst(
                    &mut commands,
                    &transport,
                    node_id,
                    endpoint,
                    claimed_device.as_ref(),
                    gap_ms,
                    warm_kelvin,
                    cool_kelvin,
                );
            } else {
                anyhow::bail!("Unknown Matter bulb test: {}", other);
            }
        }
    }

    let command_count = commands.len();
    let failed_count = commands
        .iter()
        .filter(|command| command.get("ok").and_then(Value::as_bool) == Some(false))
        .count();

    Ok(json!({
        "status": if failed_count == 0 { "ok" } else { "partial" },
        "test": test,
        "device_id": native_id,
        "node_id": node_id,
        "endpoint": endpoint,
        "claimed_capabilities": claimed_capabilities,
        "raw_capability_snapshot": raw_capability_snapshot,
        "endpoint_validation": endpoint_validation,
        "test_parameters": {
            "visual_baseline": {
                "brightness_percent": VISUAL_BASELINE_PERCENT,
                "kelvin": VISUAL_BASELINE_KELVIN,
            },
            "color_temperature": {
                "warm_kelvin": warm_kelvin,
                "cool_kelvin": cool_kelvin,
                "range_source": color_temperature_range_source,
            },
            "level_command_variants": {
                "tested": ["move_to_level", "move_to_level_with_onoff", "step", "step_with_onoff"],
            },
            "rapid_thresholds_ms": RAPID_THRESHOLDS_MS,
        },
        "command_count": command_count,
        "failed_count": failed_count,
        "commands": commands,
    }))
}

pub fn save_bulb_test_report(state: &SharedState, report: &Value) -> Result<Value> {
    let device_id = report
        .get("device_id")
        .and_then(Value::as_str)
        .context("Missing device_id")?;
    let native_id = resolve_matter_device_id(state, device_id)?;
    let report_id = report
        .get("report_id")
        .and_then(Value::as_str)
        .map(sanitize_id)
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| format!("{}-{}", native_id, now_unix_ms()));
    let received_at = now_unix_ms();

    let mut enriched = match report.as_object() {
        Some(object) => object.clone(),
        None => Map::new(),
    };
    enriched.insert("report_id".to_string(), Value::String(report_id.clone()));
    enriched.insert("device_id".to_string(), Value::String(native_id.clone()));
    enriched.insert(
        "received_at_unix_ms".to_string(),
        Value::Number(received_at.into()),
    );
    let schema_version = report
        .get("schema_version")
        .and_then(Value::as_u64)
        .unwrap_or(2);
    enriched.insert(
        "schema_version".to_string(),
        Value::Number(schema_version.into()),
    );

    let report_dir = report_dir(state).context("data_dir not configured on AppState")?;
    fs::create_dir_all(&report_dir)
        .with_context(|| format!("creating report dir {}", report_dir.display()))?;
    let report_path = report_dir.join(format!("{}.json", report_id));
    fs::write(
        &report_path,
        serde_json::to_vec_pretty(&Value::Object(enriched))
            .context("serializing Matter bulb test report")?,
    )
    .with_context(|| format!("writing {}", report_path.display()))?;

    let apply_local = report
        .get("apply_local")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let inferred_quirks = report
        .get("inferred_quirks")
        .or_else(|| report.get("quirks"));
    let capability_hints = report
        .get("capability_hints")
        .or_else(|| report.get("capabilities"));
    let control_profile = report
        .get("control_profile")
        .or_else(|| report.get("profile"));
    let mut applied_local = false;
    let mut applied_quirks = Value::Array(Vec::new());
    let mut applied_capabilities = Value::Object(Map::new());
    let mut applied_control_profile = Value::Null;

    if apply_local {
        let mut quirks = match inferred_quirks {
            Some(value) => crate::local_quirks::quirks_from_value(value)?,
            None => Vec::new(),
        };
        if let Some(preference) = recommended_color_preference_quirk(report) {
            quirks.retain(|quirk| !is_color_preference_quirk(quirk));
            quirks.push(preference);
        }
        if !quirks.is_empty() {
            let curated_quirks = curated_device_quirks(state, &native_id)?;
            quirks = crate::local_quirks::apply_quirk_override(&curated_quirks, &quirks);
        }
        let capability_override = match capability_hints {
            Some(value) => crate::local_quirks::capability_override_from_value(value)?,
            None => crate::local_quirks::LocalCapabilityOverride::default(),
        };

        if !quirks.is_empty() || !capability_override.is_empty() {
            crate::local_quirks::save_device_profile_override(
                state,
                &native_id,
                if quirks.is_empty() {
                    None
                } else {
                    Some(quirks.clone())
                },
                if capability_override.is_empty() {
                    None
                } else {
                    Some(capability_override.clone())
                },
                Some(report_id.clone()),
            )?;
            if !quirks.is_empty() {
                apply_runtime_quirks(state, &native_id, quirks.clone())?;
                applied_quirks = crate::local_quirks::quirks_to_value(&quirks);
            }
            if !capability_override.is_empty() {
                apply_runtime_capabilities(state, &native_id, &capability_override)?;
                applied_capabilities =
                    crate::local_quirks::capabilities_to_value(&capability_override);
            }
            applied_local = true;
        }
        if let Some(value) = control_profile {
            let profile: crate::control_profile::MatterControlProfile =
                serde_json::from_value(value.clone()).context("invalid Matter control profile")?;
            if profile.command_spacing_ms.value_ms > 10_000 {
                anyhow::bail!("Matter control profile command spacing exceeds 10000 ms");
            }
            crate::local_quirks::save_device_control_profile(
                state,
                &native_id,
                profile.clone(),
                Some(report_id.clone()),
            )?;
            apply_runtime_control_profile(state, &native_id, profile.clone())?;
            applied_control_profile = serde_json::to_value(profile)?;
            applied_local = true;
        }
    };

    Ok(json!({
        "status": "saved",
        "report_id": report_id,
        "device_id": native_id,
        "local_path": report_path.to_string_lossy(),
        "applied_local": applied_local,
        "applied_quirks": applied_quirks,
        "applied_capabilities": applied_capabilities,
        "applied_control_profile": applied_control_profile,
    }))
}

fn recommended_color_preference_quirk(report: &Value) -> Option<rhythm_devices::DeviceQuirk> {
    match report
        .pointer("/recommended_control_strategy/color_command")
        .and_then(Value::as_str)
    {
        Some("hue_saturation") => Some(rhythm_devices::DeviceQuirk::NeedsHueSaturationNotCt),
        Some("xy") => Some(rhythm_devices::DeviceQuirk::NeedsXyNotCt),
        Some("color_temperature") => Some(rhythm_devices::DeviceQuirk::Other(
            rhythm_devices::quirks::PREFER_COLOR_TEMPERATURE_QUIRK.to_string(),
        )),
        _ => None,
    }
}

fn is_color_preference_quirk(quirk: &rhythm_devices::DeviceQuirk) -> bool {
    match quirk {
        rhythm_devices::DeviceQuirk::NeedsHueSaturationNotCt
        | rhythm_devices::DeviceQuirk::NeedsXyNotCt => true,
        rhythm_devices::DeviceQuirk::Other(value) => {
            value == rhythm_devices::quirks::PREFER_COLOR_TEMPERATURE_QUIRK
        }
        _ => false,
    }
}

fn curated_device_quirks(
    state: &SharedState,
    device_id: &str,
) -> Result<Vec<rhythm_devices::DeviceQuirk>> {
    let (node_id, _) = crate::lifecycle::parse_device_id(device_id)
        .with_context(|| format!("Invalid Matter device ID: {}", device_id))?;
    let hub_data = get_hub_data(state)?;
    let transport = hub_data
        .transport
        .get()
        .context("Matter transport not initialized")?;
    let device = transport.probe_light(node_id).with_context(|| {
        format!(
            "probing Matter device {} before applying local quirks",
            node_id
        )
    })?;
    let mut caps = crate::commissioning::build_device_capabilities(&device);
    let mut quirks = crate::commissioning::build_device_quirks(&device);
    let cloud_profiles = hub_data
        .cloud_profiles
        .lock()
        .map_err(|_| anyhow::anyhow!("Matter cloud profile lock"))?;
    cloud_profiles.apply_to_device(&device, &mut caps, &mut quirks);
    Ok(quirks)
}

fn get_hub_data(state: &SharedState) -> Result<Arc<MatterHubData>> {
    let hub_key = HubKey::new(HubType::new("matter"), "local");
    state
        .lock()
        .map_err(|_| anyhow::anyhow!("lock"))?
        .hubs
        .get(&hub_key)
        .ok_or_else(|| anyhow::anyhow!("Matter hub not connected"))?
        .data::<Arc<MatterHubData>>()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Matter hub data missing"))
}

fn resolve_matter_device_id(state: &SharedState, device_id: &str) -> Result<String> {
    if crate::lifecycle::parse_device_id(device_id).is_some() {
        return Ok(device_id.to_string());
    }

    let matter_hub_key = HubKey::new(HubType::new("matter"), "local");
    let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let device = state
        .canonical_registry
        .get(device_id)
        .ok_or_else(|| anyhow::anyhow!("Invalid Matter device ID: {}", device_id))?;
    let endpoint = device
        .active_endpoints()
        .find(|endpoint| {
            endpoint.hub_key == matter_hub_key
                && crate::lifecycle::parse_device_id(&endpoint.native_id).is_some()
        })
        .ok_or_else(|| anyhow::anyhow!("Device '{}' has no Matter endpoint", device_id))?;
    Ok(endpoint.native_id.clone())
}

fn apply_runtime_quirks(
    state: &SharedState,
    device_id: &str,
    quirks: Vec<rhythm_devices::DeviceQuirk>,
) -> Result<()> {
    let hub_data = get_hub_data(state)?;
    let mut device_quirks = hub_data
        .device_quirks
        .lock()
        .map_err(|_| anyhow::anyhow!("Matter quirk cache lock"))?;
    device_quirks.insert(device_id.to_string(), quirks);
    Ok(())
}

fn apply_runtime_control_profile(
    state: &SharedState,
    device_id: &str,
    profile: crate::control_profile::MatterControlProfile,
) -> Result<()> {
    let hub_data = get_hub_data(state)?;
    hub_data
        .device_profiles
        .lock()
        .map_err(|_| anyhow::anyhow!("Matter control profile cache lock"))?
        .insert(device_id.to_string(), profile.clone());
    hub_data
        .local_overrides
        .lock()
        .map_err(|_| anyhow::anyhow!("Matter local profile cache lock"))?
        .control_profiles
        .insert(device_id.to_string(), profile);
    Ok(())
}

fn apply_runtime_capabilities(
    state: &SharedState,
    device_id: &str,
    override_caps: &crate::local_quirks::LocalCapabilityOverride,
) -> Result<()> {
    let hub_data = get_hub_data(state)?;
    let mut device_caps = hub_data
        .device_caps
        .lock()
        .map_err(|_| anyhow::anyhow!("Matter capability cache lock"))?;
    if let Some(caps) = device_caps.get_mut(device_id) {
        crate::local_quirks::apply_capability_override(caps, override_caps);
    }
    Ok(())
}

fn report_dir(state: &SharedState) -> Option<PathBuf> {
    let data_dir = state.lock().ok()?.data_dir.clone();
    if data_dir.is_empty() {
        return None;
    }
    Some(
        Path::new(&data_dir)
            .join("matter")
            .join("bulb-test-reports"),
    )
}

fn claimed_capabilities(
    node_id: u64,
    endpoint: u16,
    device: Option<&CommissionedDevice>,
    probe_error: Option<&str>,
    raw_snapshot: Option<&Value>,
) -> Value {
    let supports_ct = supports_color_mode(device, MatterColorMode::ColorTemperature);
    let supports_xy = supports_color_mode(device, MatterColorMode::Xy);
    let supports_hue_sat = supports_color_mode(device, MatterColorMode::HueSaturation);
    let color_modes = device
        .map(|device| {
            device
                .color_modes
                .iter()
                .map(|mode| match mode {
                    MatterColorMode::HueSaturation => "hue_saturation",
                    MatterColorMode::Xy => "xy",
                    MatterColorMode::ColorTemperature => "color_temperature",
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let (fallback_min_mireds, fallback_max_mireds) = device
        .and_then(|device| Some((device.max_kelvin?, device.min_kelvin?)))
        .map(|(cool_kelvin, warm_kelvin)| {
            (kelvin_to_mireds(cool_kelvin), kelvin_to_mireds(warm_kelvin))
        })
        .unwrap_or((None, None));
    let usable_ct_range = usable_color_temperature_range(device);
    let level_control = raw_snapshot.and_then(|value| value.get("level_control"));
    let color_control = raw_snapshot.and_then(|value| value.get("color_control"));
    let physical_min_mireds = color_control
        .and_then(|value| value.get("color_temp_physical_min_mireds"))
        .cloned()
        .or_else(|| fallback_min_mireds.map(Value::from))
        .unwrap_or(Value::Null);
    let physical_max_mireds = color_control
        .and_then(|value| value.get("color_temp_physical_max_mireds"))
        .cloned()
        .or_else(|| fallback_max_mireds.map(Value::from))
        .unwrap_or(Value::Null);
    let unavailable_raw_claims = if raw_snapshot.is_some() {
        Value::Array(Vec::new())
    } else {
        Value::Array(
            [
                "endpoint_list",
                "server_clusters",
                "accepted_command_lists",
                "attribute_lists",
                "level_control_feature_map",
                "color_control_feature_map",
                "current_level",
                "current_x",
                "current_y",
                "current_hue",
                "current_saturation",
            ]
            .into_iter()
            .map(Value::from)
            .collect(),
        )
    };

    json!({
        "node_id": node_id,
        "selected_endpoint": endpoint,
        "matter_vendor_id": device.map(|device| device.vendor_id),
        "matter_product_id": device.map(|device| device.product_id),
        "matter_vendor_name": device.map(|device| device.vendor_name.clone()),
        "matter_product_name": device.map(|device| device.product_name.clone()),
        "device_type": if device.is_some() { json!("light") } else { Value::Null },
        "probe_succeeded": device.is_some(),
        "probe_error": probe_error,
        "endpoint_list": raw_snapshot.and_then(|value| value.get("endpoint_list")).cloned().unwrap_or(Value::Null),
        "light_endpoint": device.map(|device| device.light_endpoint),
        "endpoint_matches_light_endpoint": device.map(|device| device.light_endpoint == endpoint),
        "server_clusters": raw_snapshot.and_then(|value| value.get("server_clusters")).cloned().unwrap_or(Value::Null),
        "client_clusters": raw_snapshot.and_then(|value| value.get("client_clusters")).cloned().unwrap_or(Value::Null),
        "device_type_list": raw_snapshot.and_then(|value| value.get("device_type_list")).cloned().unwrap_or(Value::Null),
        "accepted_command_lists": raw_snapshot.and_then(|value| value.get("accepted_command_lists")).cloned().unwrap_or(Value::Null),
        "attribute_lists": raw_snapshot.and_then(|value| value.get("attribute_lists")).cloned().unwrap_or(Value::Null),
        "onoff": device.is_some(),
        "level_control": device.is_some(),
        "color_temperature": supports_ct,
        "xy_color": supports_xy,
        "hue_saturation": supports_hue_sat,
        "transitions": true,
        "color_modes": color_modes.clone(),
        "level_control_feature_map": level_control.and_then(|value| value.get("feature_map")).cloned().unwrap_or(Value::Null),
        "color_control_feature_map": color_control.and_then(|value| value.get("feature_map")).cloned().unwrap_or(Value::Null),
        "color_capabilities": color_control.and_then(|value| value.get("color_capabilities")).cloned().unwrap_or_else(|| json!(color_modes)),
        "color_temp_physical_min_mireds": physical_min_mireds,
        "color_temp_physical_max_mireds": physical_max_mireds,
        "min_kelvin": device.and_then(|device| device.min_kelvin),
        "max_kelvin": device.and_then(|device| device.max_kelvin),
        "usable_min_kelvin": usable_ct_range.map(|(warm_kelvin, _)| warm_kelvin),
        "usable_max_kelvin": usable_ct_range.map(|(_, cool_kelvin)| cool_kelvin),
        "ct_range_valid_for_testing": usable_ct_range.is_some(),
        "current_level": level_control.and_then(|value| value.get("current_level")).cloned().unwrap_or(Value::Null),
        "current_x": color_control.and_then(|value| value.get("current_x")).cloned().unwrap_or(Value::Null),
        "current_y": color_control.and_then(|value| value.get("current_y")).cloned().unwrap_or(Value::Null),
        "current_hue": color_control.and_then(|value| value.get("current_hue")).cloned().unwrap_or(Value::Null),
        "current_saturation": color_control.and_then(|value| value.get("current_saturation")).cloned().unwrap_or(Value::Null),
        "raw_attribute_reads_available": raw_snapshot.is_some(),
        "unavailable_raw_claims": unavailable_raw_claims,
    })
}

fn endpoint_validation(endpoint: u16, device: Option<&CommissionedDevice>) -> Value {
    match device {
        Some(device) => json!({
            "selected_endpoint": endpoint,
            "claimed_light_endpoint": device.light_endpoint,
            "is_light_endpoint": endpoint == device.light_endpoint,
        }),
        None => json!({
            "selected_endpoint": endpoint,
            "claimed_light_endpoint": Value::Null,
            "is_light_endpoint": Value::Null,
            "validation_error": "probe_failed",
        }),
    }
}

fn color_temperature_targets(device: Option<&CommissionedDevice>) -> (u16, u16, &'static str) {
    let (low_kelvin, high_kelvin, source) =
        if let Some((low_kelvin, high_kelvin)) = usable_color_temperature_range(device) {
            (low_kelvin, high_kelvin, "device_physical_range")
        } else {
            (
                DEFAULT_WARM_KELVIN,
                DEFAULT_COOL_KELVIN,
                "default_sane_range",
            )
        };

    let span = high_kelvin.saturating_sub(low_kelvin);
    if span < 200 {
        return (low_kelvin, high_kelvin, source);
    }
    let margin = (span / 10).clamp(50, 500);
    (
        low_kelvin.saturating_add(margin),
        high_kelvin.saturating_sub(margin),
        source,
    )
}

fn usable_color_temperature_range(device: Option<&CommissionedDevice>) -> Option<(u16, u16)> {
    let low_kelvin = device?.min_kelvin?;
    let high_kelvin = device?.max_kelvin?;
    let (low_kelvin, high_kelvin) = if low_kelvin <= high_kelvin {
        (low_kelvin, high_kelvin)
    } else {
        (high_kelvin, low_kelvin)
    };

    if low_kelvin < MIN_REASONABLE_CT_KELVIN || high_kelvin > MAX_REASONABLE_CT_KELVIN {
        return None;
    }

    (high_kelvin > low_kelvin).then_some((low_kelvin, high_kelvin))
}

fn supports_color_mode(device: Option<&CommissionedDevice>, mode: MatterColorMode) -> bool {
    device
        .map(|device| device.color_modes.contains(&mode))
        .unwrap_or(false)
}

fn kelvin_to_mireds(kelvin: u16) -> Option<u16> {
    if kelvin == 0 {
        None
    } else {
        Some((1_000_000u32 / kelvin as u32) as u16)
    }
}

fn set_visual_baseline(
    commands: &mut Vec<Value>,
    transport: &Arc<dyn MatterTransport>,
    node_id: u64,
    endpoint: u16,
    device: Option<&CommissionedDevice>,
) {
    run_light_command(
        commands,
        transport,
        node_id,
        endpoint,
        "baseline_set_on_off_true",
        "OnOff.On",
        json!({"on": true}),
        false,
        || transport.set_on_off(node_id, endpoint, true),
    );
    sleep_ms(100);
    run_brightness_command(
        commands,
        transport,
        node_id,
        endpoint,
        "baseline_set_brightness_70",
        crate::clusters::brightness_to_level(VISUAL_BASELINE_PERCENT),
        None,
        false,
    );
    sleep_ms(150);
    if supports_color_mode(device, MatterColorMode::ColorTemperature) || device.is_none() {
        run_color_temperature_command(
            commands,
            transport,
            node_id,
            endpoint,
            "baseline_set_neutral_white",
            VISUAL_BASELINE_KELVIN,
            None,
            false,
        );
    } else if supports_color_mode(device, MatterColorMode::Xy) {
        run_xy_command(
            commands,
            transport,
            node_id,
            endpoint,
            "baseline_set_neutral_white_xy",
            XY_NEUTRAL_WHITE,
            false,
        );
    } else if supports_color_mode(device, MatterColorMode::HueSaturation) {
        run_hue_saturation_command(
            commands,
            transport,
            node_id,
            endpoint,
            "baseline_set_neutral_white_hue_sat",
            HS_NEUTRAL_WHITE,
            false,
        );
    }
    sleep_ms(350);
}

fn run_power_cycle_setup(
    commands: &mut Vec<Value>,
    transport: &Arc<dyn MatterTransport>,
    node_id: u64,
    endpoint: u16,
    warm_kelvin: u16,
) {
    run_light_command(
        commands,
        transport,
        node_id,
        endpoint,
        "power_cycle_setup_set_on",
        "OnOff.On",
        json!({"on": true}),
        false,
        || transport.set_on_off(node_id, endpoint, true),
    );
    sleep_ms(150);
    run_brightness_command(
        commands,
        transport,
        node_id,
        endpoint,
        "power_cycle_setup_brightness_50",
        crate::clusters::brightness_to_level(POWER_CYCLE_SETUP_PERCENT),
        None,
        false,
    );
    sleep_ms(200);
    run_color_temperature_command(
        commands,
        transport,
        node_id,
        endpoint,
        "power_cycle_setup_warm_white",
        warm_kelvin,
        None,
        true,
    );
}

#[allow(clippy::too_many_arguments)]
fn run_brightness_command(
    commands: &mut Vec<Value>,
    transport: &Arc<dyn MatterTransport>,
    node_id: u64,
    endpoint: u16,
    name: &str,
    level: u8,
    transition_ms: Option<u32>,
    delayed_readback: bool,
) {
    run_light_command(
        commands,
        transport,
        node_id,
        endpoint,
        name,
        "LevelControl.MoveToLevelWithOnOff",
        json!({
            "level": level,
            "transition_ms": transition_ms,
        }),
        delayed_readback,
        || transport.set_brightness(node_id, endpoint, level, transition_ms),
    );
}

#[allow(clippy::too_many_arguments)]
fn run_level_variant_command(
    commands: &mut Vec<Value>,
    transport: &Arc<dyn MatterTransport>,
    node_id: u64,
    endpoint: u16,
    name: &str,
    command: MatterLevelCommandVariant,
    level_or_step: u8,
    step_mode: Option<MatterLevelStepMode>,
    transition_ms: Option<u32>,
    delayed_readback: bool,
) {
    run_light_command(
        commands,
        transport,
        node_id,
        endpoint,
        name,
        level_command_name(command),
        json!({
            "level_or_step": level_or_step,
            "step_mode": step_mode,
            "transition_ms": transition_ms,
        }),
        delayed_readback,
        || {
            transport.run_level_command(
                node_id,
                endpoint,
                command,
                level_or_step,
                step_mode,
                transition_ms,
            )
        },
    );
}

fn level_command_name(command: MatterLevelCommandVariant) -> &'static str {
    match command {
        MatterLevelCommandVariant::MoveToLevel => "LevelControl.MoveToLevel",
        MatterLevelCommandVariant::MoveToLevelWithOnOff => "LevelControl.MoveToLevelWithOnOff",
        MatterLevelCommandVariant::Step => "LevelControl.Step",
        MatterLevelCommandVariant::StepWithOnOff => "LevelControl.StepWithOnOff",
    }
}

#[allow(clippy::too_many_arguments)]
fn run_color_temperature_command(
    commands: &mut Vec<Value>,
    transport: &Arc<dyn MatterTransport>,
    node_id: u64,
    endpoint: u16,
    name: &str,
    kelvin: u16,
    transition_ms: Option<u32>,
    delayed_readback: bool,
) {
    run_light_command(
        commands,
        transport,
        node_id,
        endpoint,
        name,
        "ColorControl.MoveToColorTemperature",
        json!({
            "kelvin": kelvin,
            "transition_ms": transition_ms,
        }),
        delayed_readback,
        || transport.set_color_temperature(node_id, endpoint, kelvin, transition_ms),
    );
}

fn run_xy_command(
    commands: &mut Vec<Value>,
    transport: &Arc<dyn MatterTransport>,
    node_id: u64,
    endpoint: u16,
    name: &str,
    xy: (f32, f32),
    delayed_readback: bool,
) {
    run_light_command(
        commands,
        transport,
        node_id,
        endpoint,
        name,
        "ColorControl.MoveToColor",
        json!({
            "x": xy.0,
            "y": xy.1,
            "transition_ms": Value::Null,
        }),
        delayed_readback,
        || transport.set_xy(node_id, endpoint, xy.0, xy.1, None),
    );
}

fn run_hue_saturation_command(
    commands: &mut Vec<Value>,
    transport: &Arc<dyn MatterTransport>,
    node_id: u64,
    endpoint: u16,
    name: &str,
    hue_saturation: (u8, u8),
    delayed_readback: bool,
) {
    run_light_command(
        commands,
        transport,
        node_id,
        endpoint,
        name,
        "ColorControl.MoveToHueAndSaturation",
        json!({
            "hue": hue_saturation.0,
            "saturation": hue_saturation.1,
            "transition_ms": Value::Null,
        }),
        delayed_readback,
        || {
            transport.set_hue_saturation(
                node_id,
                endpoint,
                hue_saturation.0,
                hue_saturation.1,
                None,
            )
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn run_light_command<F>(
    commands: &mut Vec<Value>,
    transport: &Arc<dyn MatterTransport>,
    node_id: u64,
    endpoint: u16,
    name: &str,
    command: &str,
    payload: Value,
    delayed_readback: bool,
    f: F,
) where
    F: FnOnce() -> Result<()>,
{
    let readback_before = readback_snapshot(transport, node_id, endpoint);
    let started_at_unix_ms = now_unix_ms();
    let (ok, command_status, error) = match f() {
        Ok(()) => (true, "success".to_string(), None),
        Err(error) => (false, "error".to_string(), Some(error.to_string())),
    };
    let readback_after_immediate = readback_snapshot(transport, node_id, endpoint);
    let mut entry = json!({
        "name": name,
        "command": command,
        "payload": payload,
        "ok": ok,
        "command_status": command_status,
        "started_at_unix_ms": started_at_unix_ms,
        "readback_before": readback_before,
        "readback_after_immediate": readback_after_immediate,
    });
    if let Some(error) = error {
        entry["error"] = Value::String(error);
    }
    if delayed_readback {
        sleep_ms(500);
        entry["readback_after_500ms"] = readback_snapshot(transport, node_id, endpoint);
        sleep_ms(1000);
        entry["readback_after_1500ms"] = readback_snapshot(transport, node_id, endpoint);
    }
    commands.push(entry);
}

#[allow(clippy::too_many_arguments)]
fn run_rapid_burst(
    commands: &mut Vec<Value>,
    transport: &Arc<dyn MatterTransport>,
    node_id: u64,
    endpoint: u16,
    device: Option<&CommissionedDevice>,
    gap_ms: u64,
    warm_kelvin: u16,
    cool_kelvin: u16,
) {
    if supports_color_mode(device, MatterColorMode::HueSaturation) {
        run_rapid_hue_saturation_burst(commands, transport, node_id, endpoint, gap_ms);
        return;
    }
    if supports_color_mode(device, MatterColorMode::Xy) {
        run_rapid_xy_burst(commands, transport, node_id, endpoint, gap_ms);
        return;
    }
    if supports_color_mode(device, MatterColorMode::ColorTemperature) {
        run_rapid_color_temperature_burst(
            commands,
            transport,
            node_id,
            endpoint,
            gap_ms,
            warm_kelvin,
            cool_kelvin,
        );
        return;
    }
    run_rapid_brightness_burst(commands, transport, node_id, endpoint, gap_ms);
}

fn run_rapid_hue_saturation_burst(
    commands: &mut Vec<Value>,
    transport: &Arc<dyn MatterTransport>,
    node_id: u64,
    endpoint: u16,
    gap_ms: u64,
) {
    let colors = [
        ("red", HS_RED),
        ("green", HS_GREEN),
        ("blue", HS_BLUE),
        ("red", HS_RED),
    ];
    run_rapid_burst_with(
        commands,
        transport,
        node_id,
        endpoint,
        gap_ms,
        |index| {
            let (label, (hue, saturation)) = colors[index];
            let payload = json!({
                "color": label,
                "hue": hue,
                "saturation": saturation,
                "transition_ms": Value::Null,
            });
            let result = transport.set_hue_saturation(node_id, endpoint, hue, saturation, None);
            rapid_subcommand_result(
                "ColorControl.MoveToHueAndSaturation",
                label,
                payload,
                result,
            )
        },
        "hue_saturation_colors",
        "ColorControl.MoveToHueAndSaturation.burst",
        json!({
            "expected_visible_sequence": colors.map(|(label, _)| label),
        }),
    );
}

fn run_rapid_xy_burst(
    commands: &mut Vec<Value>,
    transport: &Arc<dyn MatterTransport>,
    node_id: u64,
    endpoint: u16,
    gap_ms: u64,
) {
    let colors = [
        ("red", XY_RED),
        ("green", XY_GREEN),
        ("blue", XY_BLUE),
        ("red", XY_RED),
    ];
    run_rapid_burst_with(
        commands,
        transport,
        node_id,
        endpoint,
        gap_ms,
        |index| {
            let (label, (x, y)) = colors[index];
            let payload = json!({
                "color": label,
                "x": x,
                "y": y,
                "transition_ms": Value::Null,
            });
            let result = transport.set_xy(node_id, endpoint, x, y, None);
            rapid_subcommand_result("ColorControl.MoveToColor", label, payload, result)
        },
        "xy_colors",
        "ColorControl.MoveToColor.burst",
        json!({
            "expected_visible_sequence": colors.map(|(label, _)| label),
        }),
    );
}

fn run_rapid_color_temperature_burst(
    commands: &mut Vec<Value>,
    transport: &Arc<dyn MatterTransport>,
    node_id: u64,
    endpoint: u16,
    gap_ms: u64,
    warm_kelvin: u16,
    cool_kelvin: u16,
) {
    let temperatures = [
        ("warm", warm_kelvin),
        ("cool", cool_kelvin),
        ("warm", warm_kelvin),
        ("cool", cool_kelvin),
    ];
    run_rapid_burst_with(
        commands,
        transport,
        node_id,
        endpoint,
        gap_ms,
        |index| {
            let (label, kelvin) = temperatures[index];
            let payload = json!({
                "temperature": label,
                "kelvin": kelvin,
                "transition_ms": Value::Null,
            });
            let result = transport.set_color_temperature(node_id, endpoint, kelvin, None);
            rapid_subcommand_result(
                "ColorControl.MoveToColorTemperature",
                label,
                payload,
                result,
            )
        },
        "color_temperature_steps",
        "ColorControl.MoveToColorTemperature.burst",
        json!({
            "expected_visible_sequence": temperatures.map(|(label, _)| label),
        }),
    );
}

fn run_rapid_brightness_burst(
    commands: &mut Vec<Value>,
    transport: &Arc<dyn MatterTransport>,
    node_id: u64,
    endpoint: u16,
    gap_ms: u64,
) {
    let levels = [
        crate::clusters::brightness_to_level(20),
        crate::clusters::brightness_to_level(90),
        crate::clusters::brightness_to_level(35),
        crate::clusters::brightness_to_level(75),
    ];
    run_rapid_burst_with(
        commands,
        transport,
        node_id,
        endpoint,
        gap_ms,
        |index| {
            let level = levels[index];
            let payload = json!({
                "level": level,
                "transition_ms": Value::Null,
            });
            let result = transport.set_brightness(node_id, endpoint, level, None);
            rapid_subcommand_result(
                "LevelControl.MoveToLevelWithOnOff",
                &format!("level_{}", level),
                payload,
                result,
            )
        },
        "brightness_levels",
        "LevelControl.MoveToLevelWithOnOff.burst",
        json!({
            "levels": levels,
        }),
    );
}

#[allow(clippy::too_many_arguments)]
fn run_rapid_burst_with<F>(
    commands: &mut Vec<Value>,
    transport: &Arc<dyn MatterTransport>,
    node_id: u64,
    endpoint: u16,
    gap_ms: u64,
    mut run_step: F,
    sequence_kind: &str,
    command_name: &str,
    extra_payload: Value,
) where
    F: FnMut(usize) -> Value,
{
    let readback_before = readback_snapshot(transport, node_id, endpoint);
    let started_at_unix_ms = now_unix_ms();
    let mut subcommands = Vec::new();
    for index in 0..4 {
        subcommands.push(run_step(index));
        if gap_ms > 0 && index < 3 {
            sleep_ms(gap_ms);
        }
    }
    let readback_after_immediate = readback_snapshot(transport, node_id, endpoint);
    sleep_ms(500);
    let readback_after_500ms = readback_snapshot(transport, node_id, endpoint);
    sleep_ms(1000);
    let readback_after_1500ms = readback_snapshot(transport, node_id, endpoint);
    let ok = subcommands
        .iter()
        .all(|command| command.get("ok").and_then(Value::as_bool) == Some(true));
    commands.push(json!({
        "name": format!("rapid_burst_{}ms", gap_ms),
        "command": command_name,
        "payload": {
            "gap_ms": gap_ms,
            "sequence_kind": sequence_kind,
            "details": extra_payload,
        },
        "ok": ok,
        "command_status": if ok { "success" } else { "error" },
        "started_at_unix_ms": started_at_unix_ms,
        "subcommands": subcommands,
        "readback_before": readback_before,
        "readback_after_immediate": readback_after_immediate,
        "readback_after_500ms": readback_after_500ms,
        "readback_after_1500ms": readback_after_1500ms,
    }));
}

fn rapid_subcommand_result(
    command: &str,
    label: &str,
    payload: Value,
    result: Result<()>,
) -> Value {
    match result {
        Ok(()) => json!({
            "command": command,
            "label": label,
            "payload": payload,
            "ok": true,
            "command_status": "success",
        }),
        Err(error) => json!({
            "command": command,
            "label": label,
            "payload": payload,
            "ok": false,
            "command_status": "error",
            "error": error.to_string(),
        }),
    }
}

fn rapid_threshold_ms(test: &str) -> Option<u64> {
    let raw = test.strip_prefix("rapid_")?.strip_suffix("ms")?;
    let gap_ms = raw.parse::<u64>().ok()?;
    RAPID_THRESHOLDS_MS.contains(&gap_ms).then_some(gap_ms)
}

fn read_on_off_command(
    commands: &mut Vec<Value>,
    transport: &Arc<dyn MatterTransport>,
    node_id: u64,
    endpoint: u16,
) {
    let snapshot = readback_snapshot(transport, node_id, endpoint);
    let ok = snapshot
        .get("onoff")
        .and_then(|value| value.get("ok"))
        .and_then(Value::as_bool)
        == Some(true);
    let value = snapshot
        .get("onoff")
        .and_then(|value| value.get("value"))
        .cloned();
    let error = snapshot
        .get("onoff")
        .and_then(|value| value.get("error"))
        .cloned();
    let mut entry = json!({
        "name": "read_on_off",
        "command": "OnOff.ReadAttribute.OnOff",
        "payload": {},
        "ok": ok,
        "command_status": if ok { "success" } else { "error" },
        "readback_after_immediate": snapshot,
    });
    if let Some(value) = value {
        entry["value"] = value;
    }
    if let Some(error) = error {
        entry["error"] = error;
    }
    commands.push(entry);
}

fn readback_snapshot(transport: &Arc<dyn MatterTransport>, node_id: u64, endpoint: u16) -> Value {
    if let Ok(value) = transport.read_light_state(node_id, endpoint) {
        return value;
    }

    match transport.read_on_off(node_id, endpoint) {
        Ok(value) => json!({
            "onoff": {
                "ok": true,
                "value": value,
            }
        }),
        Err(error) => json!({
            "onoff": {
                "ok": false,
                "error": error.to_string(),
            }
        }),
    }
}

#[cfg(not(test))]
fn sleep_ms(ms: u64) {
    thread::sleep(Duration::from_millis(ms));
}

#[cfg(test)]
fn sleep_ms(_ms: u64) {}

fn sanitize_id(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .collect()
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU64};
    use std::sync::{Arc, Mutex};

    use rhythm_devices::{DeviceQuirk, LightCapabilities, LightType};
    use rhythm_os::hub::{ActiveHub, HubType};
    use rhythm_os::registry::HubDeviceRegistry;
    use rhythm_os::state::{AppState, SharedState};
    use serde_json::json;

    use crate::cloud_profiles::CloudMatterProfileCatalog;
    use crate::test_support::SpyTransport;
    use crate::transport::{MatterDeviceInfo, MatterTransport};

    fn commissioned_device(min_kelvin: Option<u16>, max_kelvin: Option<u16>) -> CommissionedDevice {
        CommissionedDevice {
            node_id: 1,
            vendor_name: "Vendor".to_string(),
            product_name: "Bulb".to_string(),
            vendor_id: 1,
            product_id: 1,
            serial_number: None,
            light_endpoint: 1,
            color_modes: vec![MatterColorMode::ColorTemperature],
            min_kelvin,
            max_kelvin,
        }
    }

    fn commissioned_device_with_modes(color_modes: Vec<MatterColorMode>) -> CommissionedDevice {
        CommissionedDevice {
            node_id: 42,
            vendor_name: "Vendor".to_string(),
            product_name: "Bulb".to_string(),
            vendor_id: 1,
            product_id: 2,
            serial_number: None,
            light_endpoint: 1,
            color_modes,
            min_kelvin: Some(2000),
            max_kelvin: Some(6500),
        }
    }

    fn moes_matter_light() -> CommissionedDevice {
        CommissionedDevice {
            node_id: 42,
            vendor_name: "MOES".to_string(),
            product_name: "MOES Matter Light".to_string(),
            vendor_id: 5245,
            product_id: 1412,
            serial_number: Some("moes-cache-test".to_string()),
            light_endpoint: 1,
            color_modes: vec![
                MatterColorMode::HueSaturation,
                MatterColorMode::Xy,
                MatterColorMode::ColorTemperature,
            ],
            min_kelvin: Some(2702),
            max_kelvin: Some(6535),
        }
    }

    fn unique_data_dir() -> PathBuf {
        static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);

        std::env::temp_dir()
            .join("rhythm-matter-bulb-test")
            .join(format!(
                "{}-{}-{}",
                std::process::id(),
                now_unix_ms(),
                NEXT_TEST_DIR.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ))
    }

    fn test_state_with_transport(
        transport: Arc<SpyTransport>,
    ) -> (SharedState, Arc<MatterHubData>, PathBuf) {
        let data_dir = unique_data_dir();
        std::fs::create_dir_all(&data_dir).unwrap();

        let transport_cell = std::sync::OnceLock::new();
        let transport_obj: Arc<dyn MatterTransport> = transport;
        let _ = transport_cell.set(transport_obj);

        let mut device_caps = HashMap::new();
        device_caps.insert(
            "matter-42".to_string(),
            LightCapabilities::defaults_for(LightType::ColorTemperature),
        );

        let registry = Arc::new(Mutex::new(HubDeviceRegistry::new()));
        let (event_tx, _event_rx) = std::sync::mpsc::channel();
        let hub_data = Arc::new(MatterHubData {
            transport: transport_cell,
            capture_dir: std::sync::OnceLock::new(),
            registry,
            fabric_id: "local-test".to_string(),
            commissioned: Mutex::new(vec![MatterDeviceInfo {
                node_id: 42,
                vendor_name: "Vendor".to_string(),
                product_name: "Bulb".to_string(),
                reachable: true,
            }]),
            next_node_id: AtomicU64::new(43),
            device_caps: Mutex::new(device_caps),
            fallback_caps: Mutex::new(HashSet::new()),
            device_quirks: Mutex::new(HashMap::new()),
            device_profiles: Mutex::new(HashMap::new()),
            pending_turn_on_plans: Arc::new(Mutex::new(HashMap::new())),
            needs_audition: Arc::new(Mutex::new(HashSet::new())),
            readback: Arc::new(crate::hub_state::MatterReadbackCoordinator::default()),
            local_overrides: Mutex::new(crate::local_quirks::LocalMatterOverrides::default()),
            cloud_profiles: Mutex::new(CloudMatterProfileCatalog::default()),
            decommissioning: Mutex::new(HashSet::new()),
            recently_decommissioned: Mutex::new(HashMap::new()),
            node_proof_of_life: Arc::new(Mutex::new(HashMap::new())),
            on_off_observations: Arc::new(Mutex::new(HashMap::new())),
            attribute_report_history: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            last_turn_on_dispatch: Mutex::new(HashMap::new()),
            event_tx,
        });

        let hub_type = HubType::new("matter");
        let hub_key = HubKey::new(hub_type.clone(), "local");
        let state = Arc::new(Mutex::new(AppState::default()));
        {
            let mut state = state.lock().unwrap();
            state.data_dir = data_dir.to_string_lossy().to_string();
            state.hubs.insert(
                hub_key.clone(),
                ActiveHub {
                    hub_type,
                    hub_key: hub_key.clone(),
                    runtime: None,
                    hub_data: Box::new(hub_data.clone()),
                    registry: None,
                    discovery: None,
                    shutdown: Arc::new(AtomicBool::new(false)),
                },
            );
            state.set_hub_connected(&hub_key, true);
        }

        (state, hub_data, data_dir)
    }

    fn bulb_test_state() -> (SharedState, Arc<MatterHubData>, PathBuf) {
        let transport = Arc::new(SpyTransport::new());
        transport.set_probe_device(commissioned_device_with_modes(vec![
            MatterColorMode::ColorTemperature,
            MatterColorMode::Xy,
            MatterColorMode::HueSaturation,
        ]));
        transport.set_on_off_state(42, false);
        test_state_with_transport(transport)
    }

    #[test]
    fn color_temperature_targets_use_sane_margin_inside_valid_range() {
        let device = commissioned_device(Some(2000), Some(6500));

        assert_eq!(
            color_temperature_targets(Some(&device)),
            (2450, 6050, "device_physical_range")
        );
    }

    #[test]
    fn color_temperature_targets_ignore_impossible_device_range() {
        let device = commissioned_device(Some(15), Some(6500));

        assert_eq!(
            color_temperature_targets(Some(&device)),
            (3080, 6120, "default_sane_range")
        );
    }

    #[test]
    fn run_bulb_test_exercises_command_matrix() {
        let (state, _hub_data, _data_dir) = bulb_test_state();

        for test in [
            "identify",
            "turn_off",
            "brightness_without_on",
            "brightness_with_on",
            "level_move_to_level",
            "level_move_to_level_with_onoff",
            "level_step",
            "level_step_with_onoff",
            "dim_low",
            "dim_ramp",
            "brightness_steps",
            "color_temperature",
            "color_temperature_cool",
            "xy_color",
            "xy_green",
            "xy_blue",
            "hue_sat_red",
            "hue_sat_green",
            "hue_sat_blue",
            "ct_to_xy",
            "xy_to_ct",
            "ct_to_hue_sat",
            "hue_sat_to_ct",
            "on_level_restore",
            "power_on_behavior",
            "rapid_commands",
            "rapid_50ms",
            "read_on_off",
        ] {
            let report = run_bulb_test(
                &state,
                &json!({
                    "device_id": "matter-42",
                    "test": test,
                }),
            )
            .unwrap();

            assert_eq!(report["test"], test);
            assert_eq!(report["device_id"], "matter-42");
            assert!(
                report["command_count"].as_u64().unwrap() > 0,
                "{test} should record at least one command"
            );
            assert!(
                matches!(report["status"].as_str(), Some("ok" | "partial")),
                "{test} should return a structured report"
            );
        }
    }

    #[test]
    fn run_bulb_test_reports_unknown_test_and_missing_fields() {
        let (state, _hub_data, _data_dir) = bulb_test_state();

        let unknown = run_bulb_test(
            &state,
            &json!({
                "device_id": "matter-42",
                "test": "not_a_real_test",
            }),
        )
        .unwrap_err()
        .to_string();
        assert!(unknown.contains("Unknown Matter bulb test"));

        let missing_device = run_bulb_test(&state, &json!({"test": "identify"}))
            .unwrap_err()
            .to_string();
        assert!(missing_device.contains("Missing device_id"));

        let missing_test = run_bulb_test(&state, &json!({"device_id": "matter-42"}))
            .unwrap_err()
            .to_string();
        assert!(missing_test.contains("Missing test"));
    }

    #[test]
    fn rapid_burst_selects_best_available_color_path() {
        let transport = Arc::new(SpyTransport::new());
        let transport_obj: Arc<dyn MatterTransport> = transport.clone();
        let mut commands = Vec::new();

        let xy = commissioned_device_with_modes(vec![MatterColorMode::Xy]);
        run_rapid_burst(
            &mut commands,
            &transport_obj,
            42,
            1,
            Some(&xy),
            0,
            2700,
            6500,
        );
        assert_eq!(commands[0]["payload"]["sequence_kind"], "xy_colors");

        commands.clear();
        let ct = commissioned_device_with_modes(vec![MatterColorMode::ColorTemperature]);
        run_rapid_burst(
            &mut commands,
            &transport_obj,
            42,
            1,
            Some(&ct),
            0,
            2700,
            6500,
        );
        assert_eq!(
            commands[0]["payload"]["sequence_kind"],
            "color_temperature_steps"
        );

        commands.clear();
        let dimming_only = commissioned_device_with_modes(Vec::new());
        run_rapid_burst(
            &mut commands,
            &transport_obj,
            42,
            1,
            Some(&dimming_only),
            0,
            2700,
            6500,
        );
        assert_eq!(commands[0]["payload"]["sequence_kind"], "brightness_levels");
    }

    #[test]
    fn claimed_capabilities_include_raw_snapshot_when_available() {
        let device = commissioned_device_with_modes(vec![
            MatterColorMode::ColorTemperature,
            MatterColorMode::Xy,
        ]);
        let raw = json!({
            "endpoint_list": [1, 2],
            "server_clusters": ["OnOff", "LevelControl", "ColorControl"],
            "client_clusters": [],
            "device_type_list": ["extended_color_light"],
            "accepted_command_lists": {
                "level_control": ["MoveToLevelWithOnOff"],
                "color_control": ["MoveToColorTemperature"]
            },
            "attribute_lists": {"onoff": ["OnOff"]},
            "level_control": {
                "feature_map": 1,
                "current_level": 128
            },
            "color_control": {
                "feature_map": 2,
                "color_capabilities": 16,
                "color_temp_physical_min_mireds": 153,
                "color_temp_physical_max_mireds": 500,
                "current_x": 12000,
                "current_y": 13000,
                "current_hue": 4,
                "current_saturation": 200
            }
        });

        let claims = claimed_capabilities(42, 2, Some(&device), None, Some(&raw));

        assert_eq!(claims["selected_endpoint"], 2);
        assert_eq!(claims["endpoint_list"], json!([1, 2]));
        assert_eq!(claims["raw_attribute_reads_available"], true);
        assert_eq!(claims["unavailable_raw_claims"], json!([]));
        assert_eq!(claims["color_temperature"], true);
        assert_eq!(claims["xy_color"], true);
        assert_eq!(claims["hue_saturation"], false);
        assert_eq!(claims["current_level"], 128);
    }

    #[test]
    fn save_bulb_test_report_persists_and_applies_local_overrides() {
        let (state, hub_data, _data_dir) = bulb_test_state();

        let result = save_bulb_test_report(
            &state,
            &json!({
                "device_id": "matter-42",
                "report_id": "../Needs XY!*",
                "schema_version": 7,
                "quirks": ["needs_xy_not_ct"],
                "capability_hints": {
                    "min_brightness": 0,
                    "supports_transition": false
                }
            }),
        )
        .unwrap();

        assert_eq!(result["status"], "saved");
        assert_eq!(result["report_id"], "NeedsXY");
        assert_eq!(result["device_id"], "matter-42");
        assert_eq!(result["applied_local"], true);
        assert!(PathBuf::from(result["local_path"].as_str().unwrap()).exists());

        let quirks = hub_data.device_quirks.lock().unwrap();
        assert_eq!(
            quirks.get("matter-42"),
            Some(&vec![DeviceQuirk::NeedsXyNotCt])
        );
        drop(quirks);

        let caps = hub_data.device_caps.lock().unwrap();
        let updated = caps.get("matter-42").unwrap();
        assert_eq!(updated.min_brightness, Some(1));
        assert!(!updated.supports_transition);

        let overrides = crate::local_quirks::load_overrides_for_state(&state);
        assert_eq!(
            overrides.quirks.get("matter-42"),
            Some(&vec![DeviceQuirk::NeedsXyNotCt])
        );
        assert_eq!(
            overrides
                .capabilities
                .get("matter-42")
                .and_then(|caps| caps.min_brightness),
            Some(1)
        );
    }

    #[test]
    fn save_bulb_test_report_applies_recommended_color_strategy() {
        let (state, hub_data, _data_dir) = bulb_test_state();

        let result = save_bulb_test_report(
            &state,
            &json!({
                "device_id": "matter-42",
                "report_id": "Prefer CT",
                "schema_version": 2,
                "inferred_quirks": ["needs_xy_not_ct"],
                "recommended_control_strategy": {
                    "color_command": "color_temperature"
                }
            }),
        )
        .unwrap();

        let expected = vec![DeviceQuirk::Other(
            rhythm_devices::quirks::PREFER_COLOR_TEMPERATURE_QUIRK.to_string(),
        )];
        assert_eq!(result["applied_local"], true);
        assert_eq!(
            hub_data.device_quirks.lock().unwrap().get("matter-42"),
            Some(&expected)
        );
        assert_eq!(
            crate::local_quirks::load_overrides_for_state(&state)
                .quirks
                .get("matter-42"),
            Some(&expected)
        );
    }

    #[test]
    fn save_bulb_test_report_cannot_reintroduce_stale_moes_hs_cache() {
        let transport = Arc::new(SpyTransport::new());
        transport.set_probe_device(moes_matter_light());
        let (state, hub_data, _data_dir) = test_state_with_transport(transport);

        let result = save_bulb_test_report(
            &state,
            &json!({
                "device_id": "matter-42",
                "report_id": "Stale MOES HS",
                "schema_version": 2,
                "inferred_quirks": [
                    "needs_hue_saturation_not_ct",
                    {"command_throttle_ms": 250}
                ],
                "recommended_control_strategy": {
                    "color_command": "hue_saturation"
                }
            }),
        )
        .unwrap();

        let expected = vec![
            DeviceQuirk::CommandThrottleMs(250),
            DeviceQuirk::Other(rhythm_devices::quirks::PREFER_COLOR_TEMPERATURE_QUIRK.to_string()),
        ];
        assert_eq!(
            hub_data.device_quirks.lock().unwrap().get("matter-42"),
            Some(&expected)
        );
        assert_eq!(
            crate::local_quirks::load_overrides_for_state(&state)
                .quirks
                .get("matter-42"),
            Some(&expected)
        );
        assert_eq!(
            result["applied_quirks"],
            crate::local_quirks::quirks_to_value(&expected)
        );
    }

    #[test]
    fn save_bulb_test_report_does_not_apply_quirks_when_profile_probe_fails() {
        let transport = Arc::new(SpyTransport::new());
        let (state, hub_data, _data_dir) = test_state_with_transport(transport);

        let error = save_bulb_test_report(
            &state,
            &json!({
                "device_id": "matter-42",
                "report_id": "Unverified stale HS",
                "schema_version": 2,
                "inferred_quirks": ["needs_hue_saturation_not_ct"],
                "recommended_control_strategy": {
                    "color_command": "hue_saturation"
                }
            }),
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("probing Matter device 42 before applying local quirks"));
        assert!(hub_data
            .device_quirks
            .lock()
            .unwrap()
            .get("matter-42")
            .is_none());
        assert!(crate::local_quirks::load_overrides_for_state(&state)
            .quirks
            .get("matter-42")
            .is_none());
    }
}
