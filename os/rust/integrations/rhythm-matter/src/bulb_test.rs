//! Matter bulb tester commands and report persistence.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::HubType;
use rhythm_os::state::SharedState;
use serde_json::{json, Map, Value};

use crate::hub_state::MatterHubData;

const DEFAULT_IDENTIFY_SECS: u16 = 2;
const DEFAULT_BRIGHTNESS_LEVEL: u8 = 128;
const VISUAL_BASELINE_PERCENT: u8 = 70;
const VISUAL_BASELINE_KELVIN: u16 = 4000;
const DEFAULT_WARM_KELVIN: u16 = 2700;
const DEFAULT_COOL_KELVIN: u16 = 6500;
const LOW_DIM_PERCENT: u8 = 3;
const DIM_RAMP_START_PERCENT: u8 = 85;
const DIM_RAMP_END_PERCENT: u8 = 10;
const DIM_RAMP_TRANSITION_MS: u32 = 3000;
const XY_RED: (f32, f32) = (0.70, 0.30);
const XY_GREEN: (f32, f32) = (0.17, 0.70);
const XY_BLUE: (f32, f32) = (0.15, 0.06);

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

    let mut commands = Vec::new();
    match test {
        "identify" => {
            run_command(&mut commands, "identify", || {
                transport.identify_light(node_id, endpoint, DEFAULT_IDENTIFY_SECS)
            });
        }
        "turn_off" => {
            run_command(&mut commands, "set_on_off_false", || {
                transport.set_on_off(node_id, endpoint, false)
            });
            sleep_ms(200);
            read_on_off(&mut commands, &transport, node_id, endpoint);
        }
        "brightness_without_on" => {
            run_command(&mut commands, "set_on_off_false", || {
                transport.set_on_off(node_id, endpoint, false)
            });
            sleep_ms(250);
            run_command(&mut commands, "set_brightness_without_on", || {
                transport.set_brightness(node_id, endpoint, DEFAULT_BRIGHTNESS_LEVEL, None)
            });
            sleep_ms(350);
            read_on_off(&mut commands, &transport, node_id, endpoint);
        }
        "brightness_with_on" => {
            run_command(&mut commands, "set_on_off_false", || {
                transport.set_on_off(node_id, endpoint, false)
            });
            sleep_ms(250);
            run_command(&mut commands, "set_on_off_true", || {
                transport.set_on_off(node_id, endpoint, true)
            });
            sleep_ms(100);
            run_command(&mut commands, "set_brightness_after_on", || {
                transport.set_brightness(node_id, endpoint, DEFAULT_BRIGHTNESS_LEVEL, None)
            });
            sleep_ms(350);
            read_on_off(&mut commands, &transport, node_id, endpoint);
        }
        "dim_low" => {
            set_visual_baseline(&mut commands, &transport, node_id, endpoint);
            run_command(&mut commands, "set_brightness_low_dim", || {
                transport.set_brightness(
                    node_id,
                    endpoint,
                    crate::clusters::brightness_to_level(LOW_DIM_PERCENT),
                    None,
                )
            });
            sleep_ms(700);
            read_on_off(&mut commands, &transport, node_id, endpoint);
        }
        "dim_ramp" => {
            set_visual_baseline(&mut commands, &transport, node_id, endpoint);
            run_command(&mut commands, "set_brightness_ramp_start", || {
                transport.set_brightness(
                    node_id,
                    endpoint,
                    crate::clusters::brightness_to_level(DIM_RAMP_START_PERCENT),
                    None,
                )
            });
            sleep_ms(400);
            run_command(&mut commands, "set_brightness_ramp_down", || {
                transport.set_brightness(
                    node_id,
                    endpoint,
                    crate::clusters::brightness_to_level(DIM_RAMP_END_PERCENT),
                    Some(DIM_RAMP_TRANSITION_MS),
                )
            });
            sleep_ms(700);
            read_on_off(&mut commands, &transport, node_id, endpoint);
        }
        "brightness_steps" => {
            set_visual_baseline(&mut commands, &transport, node_id, endpoint);
            for brightness in [20, 60, 100, 40] {
                run_command(
                    &mut commands,
                    &format!("set_brightness_{}", brightness),
                    || {
                        transport.set_brightness(
                            node_id,
                            endpoint,
                            crate::clusters::brightness_to_level(brightness),
                            None,
                        )
                    },
                );
                sleep_ms(450);
            }
            read_on_off(&mut commands, &transport, node_id, endpoint);
        }
        "color_temperature" | "color_temperature_warm" => {
            set_visual_baseline(&mut commands, &transport, node_id, endpoint);
            run_command(&mut commands, "set_color_temperature", || {
                transport.set_color_temperature(node_id, endpoint, DEFAULT_WARM_KELVIN, None)
            });
        }
        "color_temperature_cool" => {
            set_visual_baseline(&mut commands, &transport, node_id, endpoint);
            run_command(&mut commands, "set_color_temperature_cool", || {
                transport.set_color_temperature(node_id, endpoint, DEFAULT_COOL_KELVIN, None)
            });
        }
        "xy_color" | "xy_red" => {
            set_visual_baseline(&mut commands, &transport, node_id, endpoint);
            run_command(&mut commands, "set_xy_red", || {
                transport.set_xy(node_id, endpoint, XY_RED.0, XY_RED.1, None)
            });
        }
        "xy_green" => {
            set_visual_baseline(&mut commands, &transport, node_id, endpoint);
            run_command(&mut commands, "set_xy_green", || {
                transport.set_xy(node_id, endpoint, XY_GREEN.0, XY_GREEN.1, None)
            });
        }
        "xy_blue" => {
            set_visual_baseline(&mut commands, &transport, node_id, endpoint);
            run_command(&mut commands, "set_xy_blue", || {
                transport.set_xy(node_id, endpoint, XY_BLUE.0, XY_BLUE.1, None)
            });
        }
        "rapid_commands" => {
            set_visual_baseline(&mut commands, &transport, node_id, endpoint);
            run_command(&mut commands, "set_brightness_40", || {
                transport.set_brightness(node_id, endpoint, 40, None)
            });
            run_command(&mut commands, "set_brightness_200", || {
                transport.set_brightness(node_id, endpoint, 200, None)
            });
            run_command(&mut commands, "set_brightness_90", || {
                transport.set_brightness(node_id, endpoint, 90, None)
            });
        }
        "read_on_off" => {
            read_on_off(&mut commands, &transport, node_id, endpoint);
        }
        other => anyhow::bail!("Unknown Matter bulb test: {}", other),
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
    enriched.insert("schema_version".to_string(), Value::Number(1.into()));

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
    let mut applied_local = false;
    let mut applied_quirks = Value::Array(Vec::new());
    let mut applied_capabilities = Value::Object(Map::new());

    if apply_local {
        let quirks = match inferred_quirks {
            Some(value) => crate::local_quirks::quirks_from_value(value)?,
            None => Vec::new(),
        };
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
    };

    Ok(json!({
        "status": "saved",
        "report_id": report_id,
        "device_id": native_id,
        "local_path": report_path.to_string_lossy(),
        "applied_local": applied_local,
        "applied_quirks": applied_quirks,
        "applied_capabilities": applied_capabilities,
    }))
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

fn set_visual_baseline(
    commands: &mut Vec<Value>,
    transport: &Arc<dyn crate::transport::MatterTransport>,
    node_id: u64,
    endpoint: u16,
) {
    run_command(commands, "baseline_set_on_off_true", || {
        transport.set_on_off(node_id, endpoint, true)
    });
    sleep_ms(100);
    run_command(commands, "baseline_set_brightness_70", || {
        transport.set_brightness(
            node_id,
            endpoint,
            crate::clusters::brightness_to_level(VISUAL_BASELINE_PERCENT),
            None,
        )
    });
    sleep_ms(150);
    run_command(commands, "baseline_set_neutral_white", || {
        transport.set_color_temperature(node_id, endpoint, VISUAL_BASELINE_KELVIN, None)
    });
    sleep_ms(350);
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

fn run_command<F>(commands: &mut Vec<Value>, name: &str, f: F)
where
    F: FnOnce() -> Result<()>,
{
    match f() {
        Ok(()) => commands.push(json!({"name": name, "ok": true})),
        Err(error) => commands.push(json!({
            "name": name,
            "ok": false,
            "error": error.to_string(),
        })),
    }
}

fn read_on_off(
    commands: &mut Vec<Value>,
    transport: &Arc<dyn crate::transport::MatterTransport>,
    node_id: u64,
    endpoint: u16,
) {
    match transport.read_on_off(node_id, endpoint) {
        Ok(value) => commands.push(json!({
            "name": "read_on_off",
            "ok": true,
            "value": value,
        })),
        Err(error) => commands.push(json!({
            "name": "read_on_off",
            "ok": false,
            "error": error.to_string(),
        })),
    }
}

fn sleep_ms(ms: u64) {
    thread::sleep(Duration::from_millis(ms));
}

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
