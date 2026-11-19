//! Room-scoped agent lighting. All device I/O uses the ordinary Rhythm router.
use super::{
    object_schema, operation, LightAssistantError as Error, LightAssistantOperationDto,
    OperationTier,
};
use crate::api_types::NodeStateDto;
use crate::commands;
use crate::scenes::{SceneApplyResponse, SceneDraftPreviewRequest, ScenePreviewRequest};
use crate::state::{current_epoch_ms, rooms_from_engine, AppState, SharedState};
use rhythm_core::runtime::handle::ReviewedLightEdit;
use rhythm_core::{
    CurveContext, LightCurveShape, LightNodeKind, LightProfile, LightProfileConfig,
    LightProfileModule, LightProfileNodeOverride, NodeSnapshot, RhythmMode, RoomModeState,
    RuntimeHandle,
};
use serde_json::{json, Value};
use std::sync::{Arc, MutexGuard};

const PLAN_LIFETIME_MS: u64 = 5 * 60 * 1000;

#[derive(Debug)]
pub struct LightPlan {
    id: String,
    expires_ms: u64,
    server: String,
    contract: String,
    operation: &'static str,
    room_id: String,
    room_mode: RhythmMode,
    global_mode: RhythmMode,
    modes: Vec<rhythm_core::ModeConfig>,
    solar_times: Option<(f32, f32)>,
    base: LightProfileConfig,
    nodes: Vec<NodeSnapshot>,
    edit: ReviewedLightEdit,
}

type Result<T> = std::result::Result<T, Error>;
fn invalid(message: impl Into<String>) -> Error {
    Error::bad_request("invalid_lighting_request", message)
}
fn stale() -> Error {
    Error::conflict(
        "lighting_plan_changed",
        "Refresh this room and review a new proposal.",
    )
}
fn unavailable(message: impl Into<String>) -> Error {
    Error::bad_request("lighting_unavailable", message)
}
fn uncertain() -> Error {
    Error {
        status: 503,
        code: "lighting_outcome_uncertain",
        message: "Read room and device state before repeating this operation.".into(),
        retryable: false,
        mutation_may_have_applied: true,
    }
}
fn lock(state: &SharedState) -> Result<MutexGuard<'_, AppState>> {
    state
        .lock()
        .map_err(|_| Error::internal("state_unavailable", "Hub state unavailable."))
}
fn string(args: &Value, key: &str) -> Result<String> {
    args[key]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 256)
        .map(str::to_owned)
        .ok_or_else(|| invalid(format!("Expected {key}.")))
}
fn num(args: &Value, key: &str, min: f64, max: f64) -> Result<f32> {
    args[key]
        .as_f64()
        .filter(|v| (min..=max).contains(v))
        .map(|v| v as f32)
        .ok_or_else(|| invalid(format!("{key} is outside the supported range.")))
}
fn int(args: &Value, key: &str, min: i64, max: i64) -> Result<i64> {
    args[key]
        .as_i64()
        .filter(|v| (min..=max).contains(v))
        .ok_or_else(|| invalid(format!("Expected integer {key} in {min}..{max}.")))
}
fn enum_arg<'a>(args: &'a Value, key: &str, values: &[&str]) -> Result<&'a str> {
    args[key]
        .as_str()
        .filter(|v| values.contains(v))
        .ok_or_else(|| invalid(format!("Unsupported {key}.")))
}
fn str_schema() -> Value {
    json!({"type":"string","minLength":1,"maxLength":256})
}
fn number_schema(min: f32, max: f32) -> Value {
    json!({"type":"number","minimum":min,"maximum":max})
}
fn int_schema(min: i32, max: i32) -> Value {
    json!({"type":"integer","minimum":min,"maximum":max})
}

fn result_schema(id: &str) -> Value {
    // Receipts are additive. Mark only fields the operation always supplies
    // as required, so clients written against an earlier hub can still parse them.
    let text = json!({"type": "string"});
    let boolean = json!({"type": "boolean"});
    let list = json!({"type": "array"});
    let (required, properties) = match id {
        "plan_room_tuning" | "plan_absorb_offset" => (
            vec![
                "plan_id",
                "operation",
                "room_id",
                "profile_id",
                "affected_rooms",
                "cleared_offsets",
                "samples",
                "requires_confirmation",
            ],
            json!({"plan_id":text,"operation":text,"room_id":text,"profile_id":text,"affected_rooms":list,
                "cleared_offsets":list,"samples":list,"requires_confirmation":boolean}),
        ),
        "apply_room_tuning" | "apply_absorb_offset" => (
            vec![
                "plan_id",
                "operation",
                "status",
                "canonical_readback_verified",
            ],
            json!({"plan_id":text,"operation":text,"status":text,"canonical_readback_verified":boolean}),
        ),
        "read_room_lighting_setup" => (
            vec!["room_id", "scenes", "schedules", "transitions", "bindings"],
            json!({"room_id":text,"scenes":list,"schedules":list,"transitions":list,"bindings":list}),
        ),
        "sample_room_curve" => (
            vec![
                "kind",
                "room_id",
                "profile_id",
                "hour",
                "brightness",
                "kelvin",
            ],
            json!({"kind":text,"room_id":text,"profile_id":text,"hour":number_schema(0.,24.),
                "brightness":int_schema(0,100),"kelvin":{"type":"integer"}}),
        ),
        _ => (
            vec![
                "room_id",
                "profile_id",
                "canonical_brightness",
                "canonical_kelvin",
                "light_readback",
            ],
            json!({"room_id":text,"profile_id":text,"canonical_brightness":int_schema(0,100),
                "canonical_kelvin":{"type":"integer"},"light_readback":{"type":"array","items":{
                    "type":"object","required":["light_id","availability","verification","observed"]}},
                "trial_id":text,"server_acknowledged":boolean,"physical_verification":text}),
        ),
    };
    json!({"type":"object","required":required,"properties":properties})
}

pub fn operations() -> Vec<LightAssistantOperationDto> {
    use OperationTier::*;
    let mut ops = Vec::new();
    let mut room_op = |id, description, tier, fields: Vec<(&str, Value)>| {
        let mut props = json!({"room_id":str_schema()});
        let mut required = vec!["room_id"];
        for (key, schema) in fields {
            required.push(key);
            props[key] = schema;
        }
        ops.push(operation(
            id,
            description,
            tier,
            object_schema(&required, props),
            result_schema(id),
        ));
    };
    room_op(
        "explain_room_output",
        "Read the selected room's resolved Day or Sleep mode, schedule, Low Glow/scene state, profile, manual offsets and observed lights. Start here when a room is too dim.",
        Read,
        vec![],
    );
    room_op(
        "read_room_lighting_setup",
        "List selectable scenes, schedules, transitions and bindings for this hub and room.",
        Read,
        vec![],
    );
    room_op(
        "sample_room_curve",
        "Sample the current lighting profile and its offsets at a local decimal hour. A sample predicts output; it does not change lights.",
        Read,
        vec![("hour",number_schema(0.,23.999))],
    );
    room_op(
        "control_room_power",
        "Turn the room on adaptively, use its configured off behavior (can enter Low Glow), or turn every managed light fully off.",
        TemporaryLightOutput,
        vec![("power",json!({"enum":["on","off","hard_off"]}))],
    );
    room_op(
        "adjust_room_brightness",
        "Adjust adaptive brightness by percentage points. For 'a little brighter', try +5. Power may turn on. Read the lights afterwards.",
        TemporaryLightOutput,
        vec![("delta_percent",int_schema(-100,100))],
    );
    room_op(
        "adjust_room_warmth",
        "Set an adaptive Kelvin target while preserving brightness. Lower Kelvin is warmer. Some profiles or lamps cannot render the requested temperature.",
        TemporaryLightOutput,
        vec![("kelvin",int_schema(500,25000))],
    );
    room_op(
        "set_room_color",
        "Show a temporary RGB color at the room's current brightness. The preview can turn lights on. Undo using the returned trial ID.",
        TemporaryLightOutput,
        vec![("red",int_schema(0,255)),("green",int_schema(0,255)),("blue",int_schema(0,255))],
    );
    room_op(
        "step_room_curve",
        "Move the active room up (brighter, cooler) or down (dimmer, warmer) one step on its profile.",
        TemporaryLightOutput,
        vec![("direction",json!({"enum":["up","down"]}))],
    );
    room_op(
        "try_room_offset",
        "Show a live time-shifted look on the room's managed lights for one minute. Use -60 for one hour earlier. Undo with the trial ID, or review Absorb to change the shared profile.",
        TemporaryLightOutput,
        vec![("offset_minutes",number_schema(-720.,720.))],
    );
    room_op(
        "set_room_offset",
        "Keep the selected room at an absolute offset from its adaptive curve. If keeping an audition, undo the trial first, then set the same offset. Set zero to resume the unshifted curve.",
        TemporaryLightOutput,
        vec![("offset_minutes",number_schema(-720.,720.))],
    );
    room_op(
        "undo_room_trial",
        "End a live preview on the selected room and restore its current Rhythm behavior. Only use the trial ID returned by the preview.",
        TemporaryLightOutput,
        vec![("trial_id",str_schema())],
    );
    room_op(
        "return_room_to_schedule",
        "Resume adaptation without issuing a power command, reset manual offsets (may turn lights on), or restore the scheduled mode's default power and look.",
        TemporaryLightOutput,
        vec![("behavior",json!({"enum":["resume","reset_offsets","scheduled_default"]}))],
    );
    room_op(
        "preview_room_scene_draft",
        "Audition an unsaved Rhythm scene in one room. Undo the trial or review a scene preview commit to keep it.",
        TemporaryLightOutput,
        vec![("scene",json!({"type":"object","required":["id","name","light"]}))],
    );
    room_op(
        "preview_room_scene",
        "Audition a discovered scene for a minute without replacing the room's saved Mood binding. Undo with the trial ID.",
        TemporaryLightOutput,
        vec![("scene_id",str_schema())],
    );
    room_op(
        "apply_room_scene",
        "Use a discovered scene temporarily for the room without replacing its saved Mood binding. Returns an undo ID and expiration time.",
        TemporaryLightOutput,
        vec![("scene_id",str_schema())],
    );
    room_op(
        "plan_room_tuning",
        "Preview a lasting room-only preference and samples at sunset and two hours later. Choose minimum_brightness (1–100 percent) or evening_brightness_width (0.2–2.0, smaller dims later). Both preserve warmth.",
        Read,
        vec![("setting",json!({"enum":["minimum_brightness","evening_brightness_width"]})),("value",number_schema(0.2,100.))],
    );
    room_op(
        "plan_absorb_offset",
        "Review saving a time-shifted look into the shared profile. This can change brightness AND warmth and clears top-level time offsets across the hub. Review the full affected scope.",
        Read,
        vec![("offset_minutes",number_schema(-720.,720.))],
    );
    for (id,description) in [("apply_room_tuning","Save the reviewed room-only tuning after the person confirms."),("apply_absorb_offset","Apply the reviewed shared profile and clear the reviewed offsets after the person confirms.")] {
        ops.push(operation(id,description,PersistentChange,object_schema(&["plan_id","confirmed"],json!({"plan_id":str_schema(),"confirmed":{"type":"boolean","const":true}})),result_schema(id)));
    }
    for (id, desc, schema) in [
        ("run_mode_transition","Run a named hub-wide Day/Sleep transition now. This can change power in multiple rooms.",object_schema(&["transition_id"], json!({"transition_id":str_schema()}))),
        ("run_schedule_transition","Run a named schedule transition for every room assigned to it. Review its power defaults first.",object_schema(&["schedule_id","transition_id"],json!({"schedule_id":str_schema(),"transition_id":str_schema()}))),
    ] {
        ops.push(operation(id,desc,TemporaryLightOutput,schema,json!({"type":"object","required":["server_acknowledged","physical_verification"]})));
    }
    ops.push(operation("set_house_lighting_mode","Switch the whole hub to Day or Sleep now. Configured transitions and room power defaults can take effect. Report the global scope before use.",TemporaryLightOutput,object_schema(&["mode"],json!({"mode":{"enum":["day","sleep"]}})),json!({"type":"object"})));
    ops
}

struct LightingContext {
    runtime: Arc<dyn RuntimeHandle>,
    room: NodeSnapshot,
    mode: RhythmMode,
    config: LightProfileConfig,
    clock: CurveContext,
}

fn context(app: &AppState, id: &str, room_state: RoomModeState) -> Result<LightingContext> {
    let runtime = app
        .hub_runtime()
        .ok_or_else(|| unavailable("Lighting runtime is offline."))?;
    if app.topology.get(id).is_none() {
        return Err(invalid("Select a discovered Rhythm room."));
    }
    let room = runtime
        .engine_effective_node_snapshot(id)
        .filter(|n| n.kind == LightNodeKind::Room)
        .ok_or_else(|| unavailable("Room is unavailable in the lighting engine."))?;
    let mut clock = rhythm_core::curve_context_for_local_datetime(
        app.solar_noon_hour(),
        app.latitude,
        app.longitude,
        app.timezone_name.as_deref(),
        commands::current_local_datetime(app.utc_offset_hours),
    );
    clock.current_hour = runtime.current_hour();
    let mode = room
        .profile_settings
        .schedule_mode(app.active_mode, clock.current_hour);
    let config = commands::light_profile_registry_from_state(app).profile_config_for_room_state(
        mode,
        room_state,
        Some(&room.profile_settings),
    );
    Ok(LightingContext {
        runtime,
        room,
        mode,
        config,
        clock,
    })
}

fn light_reports(app: &AppState, id: &str, since: Option<u64>) -> Vec<Value> {
    let Some(room) = app.topology.get(id) else {
        return Vec::new();
    };
    let now = current_epoch_ms();
    let mut reports = Vec::new();
    for binding in &room.devices {
        let Some(light) = app.canonical_registry.get(&binding.device_id) else {
            continue;
        };
        if light.device_type != rhythm_core::runtime::hub_registry::DeviceType::Light
            || light.is_removed()
        {
            continue;
        }
        let observation = app.light_observations.get(&light.id);
        let offline = light
            .active_endpoints()
            .all(|ep| app.hub_connection_status.get(&ep.hub_key) == Some(&false));
        let availability = if offline {
            "unavailable"
        } else {
            observation.map_or("unknown", |o| o.availability.as_str())
        };
        // The HA area can include lights the resident kept under HA control.
        // Their independently reported state cannot verify a Rhythm command.
        let scope = if app.platform_context == "ha_addon" {
            if app.ha_selection_allows_light(light) {
                "ha_selected"
            } else {
                "outside_ha_selection"
            }
        } else {
            "room_member"
        };
        let readback = observation.filter(|o| {
            let source_ms = o
                .source_at_epoch_ms
                .and_then(|ms| u64::try_from(ms).ok())
                .unwrap_or(o.received_at_epoch_ms);
            availability == "available"
                && (o.lights_on.is_some()
                    || o.brightness.is_some()
                    || o.kelvin.is_some()
                    || o.rgb.is_some()
                    || o.xy.is_some())
                && source_ms <= now
                && now.saturating_sub(source_ms) <= 60_000
                && since.is_none_or(|t| source_ms > t && o.received_at_epoch_ms > t)
        });
        reports.push(json!({
            "light_id": light.id,
            "availability": availability,
            "control_scope": scope,
            "verification": if since.is_some() && scope == "outside_ha_selection" { "outside_control_scope" } else if readback.is_some() { "fresh_device_readback" } else { "no_fresh_readback" },
            "observed": readback.map(|o| json!({
                "on": o.lights_on, "brightness": o.brightness, "kelvin": o.kelvin,
                "rgb": o.rgb, "xy":o.xy, "received_at_epoch_ms": o.received_at_epoch_ms,
            })),
        }));
    }
    reports
}

fn sample(config: &LightProfileConfig, ctx: &LightingContext, hour: f32, offset: f32) -> Value {
    let clock = CurveContext::new(hour, ctx.clock.solar, ctx.clock.sun_times);
    let values = LightProfile::new(config.clone()).calculate_with_offset(&clock, offset);
    let brightness =
        (f32::from(values.brightness) + ctx.room.brightness_offset).clamp(1., 100.) as u8;
    json!({"hour":hour,"brightness":brightness,"kelvin":values.kelvin,"rgb":values.rgb})
}

fn guard_adaptive(room: &NodeStateDto) -> Result<()> {
    if room.state != RoomModeState::Active || !room.rhythm_enabled || room.disabled {
        return Err(unavailable("This room is not adapting on an active profile. Inspect its scene, Low Glow, power, and schedule before adjusting the curve."));
    }
    Ok(())
}

fn describe(state: &SharedState, room: &NodeStateDto, since: Option<u64>) -> Result<Value> {
    let app = lock(state)?;
    let ctx = context(&app, &room.id, room.state)?;
    Ok(json!({
        "room_id":room.id,
        "mode":ctx.mode,
        "state":room.state,
        "profile_id":ctx.config.id,
        "schedule":room.profile_settings.light_schedule,
        "legacy_schedule":room.profile_settings.room_schedule,
        "profile_override":room.profile_settings.profile_overrides.get(&ctx.config.id),
        "mood_scene_id":room.profile_settings.mood_scene_id,
        "adaptation_enabled":room.rhythm_enabled && !room.disabled && app.light_breaker_enabled && app.rhythm_automation_allowed_for_node(&room.id),
        "light_breaker_enabled":app.light_breaker_enabled,
        "canonical_power":room.lights_on,
        "canonical_brightness":room.brightness,
        "canonical_kelvin":room.kelvin,
        "time_offset_minutes":room.time_offset,
        "brightness_offset":room.brightness_offset,
        "pending_dispatch":room.pending_dispatch,
        "light_readback":light_reports(&app,&room.id,since)
    }))
}

fn setup(state: &SharedState, room: &NodeStateDto) -> Result<Value> {
    let scenes = commands::build_scenes_for_target(state, Some(&room.id))
        .map_err(|_| unavailable("Could not read scenes."))?;
    let scenes: Value =
        serde_json::from_str(&scenes).map_err(|_| unavailable("Could not read scene list."))?;
    let choices: Vec<_> = scenes["scenes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| json!({"id":s["id"],"name":s["name"]}))
        .collect();
    let app = lock(state)?;
    let bindings: Vec<_> = app
        .topology
        .input_bindings()
        .iter()
        .filter(|b| app.topology.device_parent_room_id(&b.source_node_id) == Some(&room.id))
        .collect();
    Ok(
        json!({"room_id":room.id,"scenes":choices,"native_scene_discovery_failed":scenes["native_discovery_failed"],"schedules":app.light_schedule_configs(),"transitions":app.mode_transition_configs(),"bindings":bindings,"mode":app.active_mode,"mode_settings":app.mode_configs()}),
    )
}

fn plan(state: &SharedState, room: &NodeStateDto, id: &str, args: &Value) -> Result<Value> {
    guard_adaptive(room)?;
    let mut app = lock(state)?;
    if !app.rhythm_automation_allowed_for_node(&room.id) {
        return Err(unavailable(
            "This room's automation is managed by another integration.",
        ));
    }
    let ctx = context(&app, &room.id, room.state)?;
    let base = app
        .light_profile_config(&ctx.config.id)
        .cloned()
        .ok_or_else(stale)?;
    let mut next = ctx.config.clone();
    let mut next_offset = ctx.room.time_offset_minutes;
    let (edit, apply_id) = if id == "plan_absorb_offset" {
        let offset = num(args, "offset_minutes", -720., 720.)?;
        if base.curve != ctx.config.curve || offset.abs() < 0.001 {
            return Err(unavailable("Absorb needs a nonzero shift on a shared adaptive curve without a room curve override."));
        }
        let (sunrise, sunset) = ctx
            .clock
            .sun_times
            .map(|s| (s.sunrise, s.sunset))
            .unwrap_or((
                rhythm_core::config::FALLBACK_SUNRISE_HOUR,
                rhythm_core::config::FALLBACK_SUNSET_HOUR,
            ));
        let mut config = commands::absorb_light_profile_time_offset(
            &base,
            ctx.clock.current_hour,
            offset,
            sunrise,
            sunset,
        )
        .ok_or_else(|| {
            unavailable("This offset cannot be absorbed on the current side of the curve.")
        })?;
        config.normalize_float_precision();
        if config == base {
            return Err(unavailable("This shift produces no profile change."));
        }
        next.curve = config.curve.clone();
        next_offset = 0.;
        (ReviewedLightEdit::Absorb { config }, "apply_absorb_offset")
    } else {
        let mut change = LightProfileNodeOverride::default();
        match enum_arg(
            args,
            "setting",
            &["minimum_brightness", "evening_brightness_width"],
        )? {
            "minimum_brightness" => {
                let value = int(args, "value", 1, i64::from(next.max_brightness))? as u8;
                next.min_brightness = value;
                change.min_brightness = Some(value);
            }
            _ => {
                let width = num(args, "value", 0.2, 2.)?;
                let LightCurveShape::SuperGaussian {
                    width_right_bri, ..
                } = &mut next.curve
                else {
                    return Err(unavailable("This profile has no evening brightness ramp."));
                };
                *width_right_bri = width;
                next.normalize_float_precision();
                change.curve = Some(next.curve.clone());
            }
        }
        if next == ctx.config {
            return Err(unavailable("The room already uses this setting."));
        }
        (
            ReviewedLightEdit::RoomOverride {
                room_id: room.id.clone(),
                change,
            },
            "apply_room_tuning",
        )
    };
    let sunset = ctx
        .clock
        .sun_times
        .map_or(rhythm_core::config::FALLBACK_SUNSET_HOUR, |s| s.sunset);
    let samples:Vec<_>=[sunset,(sunset+2.).rem_euclid(24.)].iter().map(|hour|json!({"before":sample(&ctx.config,&ctx,*hour,ctx.room.time_offset_minutes),"after":sample(&next,&ctx,*hour,next_offset)})).collect();
    let mut nodes: Vec<_> = ctx
        .runtime
        .engine_all_node_snapshots()
        .into_iter()
        .filter(|n| edit.includes(n))
        .collect();
    nodes.sort_by(|a, b| a.id.cmp(&b.id));
    // The reviewed room and its inheritance must match the same engine snapshot used to calculate the preview.
    if !nodes.iter().any(|n| n == &ctx.room) {
        return Err(stale());
    }
    let mut offsets = Vec::new();
    let mut affected_rooms = Vec::new();
    let registry = commands::light_profile_registry_from_state(&app);
    for node in &nodes {
        if crate::topology::is_internal_light_node_id(&node.id) {
            continue;
        }
        if matches!(edit, ReviewedLightEdit::Absorb { .. })
            && node.parent_id.is_none()
            && node.time_offset_minutes != 0.
        {
            offsets.push(json!({"node_id":node.id,"before_minutes":node.time_offset_minutes,"after_minutes":0}));
        }
        // Include rooms that can use this profile in either mode, including
        // currently off rooms, so the person can review future scope.
        if node.kind.is_room()
            && (matches!(edit, ReviewedLightEdit::RoomOverride { .. })
                || RhythmMode::ALL.iter().any(|m| {
                    [
                        RoomModeState::Active,
                        RoomModeState::Mood,
                        RoomModeState::Standby,
                        RoomModeState::Wake,
                        RoomModeState::Warning,
                    ]
                    .iter()
                    .any(|st| {
                        registry
                            .profile_config_for_room_state(*m, *st, Some(&node.profile_settings))
                            .id
                            == base.id
                    })
                }))
        {
            affected_rooms.push(json!({"room_id":node.id,"name":node.name}));
        }
    }
    let now = current_epoch_ms();
    let token = uuid::Uuid::new_v4().to_string();
    let expires = now + PLAN_LIFETIME_MS;
    let contract = super::light_assistant_contract_sha256();
    let preview = json!({"plan_id":token,"operation":apply_id,"contract_sha256":contract,"server_instance_id":app.server_instance_id,"expires_at_epoch_ms":expires,"requires_confirmation":true,"room_id":room.id,"profile_id":base.id,"scope":if id=="plan_absorb_offset"{"shared_profile"}else{"room_profile_override"},"affected_rooms":affected_rooms,"cleared_offsets":offsets,"before_profile":ctx.config,"after_profile":next,"samples":samples});
    let proposal = LightPlan {
        id: token,
        expires_ms: expires,
        server: app.server_instance_id.clone(),
        contract,
        operation: apply_id,
        room_id: room.id.clone(),
        room_mode: ctx.mode,
        global_mode: app.active_mode,
        modes: app.mode_configs(),
        solar_times: ctx.clock.sun_times.map(|s| (s.sunrise, s.sunset)),
        base,
        nodes,
        edit,
    };
    app.assistant_light_plans
        .retain(|p| p.expires_ms > now && !(p.operation == apply_id && p.room_id == room.id));
    if app.assistant_light_plans.len() >= 16 {
        app.assistant_light_plans.pop_front();
    }
    app.assistant_light_plans.push_back(proposal);
    Ok(preview)
}

fn apply(state: &SharedState, id: &str, args: &Value) -> Result<Value> {
    if args["confirmed"] != true {
        return Err(invalid("Review this plan and explicitly confirm it first."));
    }
    let token = string(args, "plan_id")?;
    let mut app = lock(state)?;
    let index = app
        .assistant_light_plans
        .iter()
        .position(|p| p.id == token && p.operation == id)
        .ok_or_else(stale)?;
    let plan = app.assistant_light_plans.remove(index).ok_or_else(stale)?;
    if plan.expires_ms <= current_epoch_ms()
        || plan.server != app.server_instance_id
        || plan.contract != super::light_assistant_contract_sha256()
        || app.mode_configs() != plan.modes
        || app.active_mode != plan.global_mode
        || app.light_profile_config(&plan.base.id) != Some(&plan.base)
    {
        return Err(stale());
    }
    if !app.rhythm_automation_allowed_for_node(&plan.room_id) {
        return Err(stale());
    }
    let ctx = context(&app, &plan.room_id, RoomModeState::Active)?;
    if ctx.mode != plan.room_mode
        || ctx.config.id != plan.base.id
        || ctx.clock.sun_times.map(|s| (s.sunrise, s.sunset)) != plan.solar_times
    {
        return Err(stale());
    }
    // Runtime sources (buttons, integrations and ticks) can mutate offsets
    // without acquiring AppState. Compare and commit under the engine lock too.
    let changed = ctx
        .runtime
        .apply_reviewed_light_edit(&plan.nodes, &plan.base, &plan.edit)
        .map_err(|_| unavailable("The runtime cannot apply this reviewed edit."))?;
    if !changed {
        return Err(stale());
    }
    if let ReviewedLightEdit::Absorb { config } = &plan.edit {
        app.set_light_profile_config(config.clone());
        if config.id == app.active_mode_profile_id() {
            app.sync_active_mode_runtime_overrides();
        }
    }
    app.invalidate_queued_light_dispatches();
    if let Some(storage) = &app.storage {
        if matches!(plan.edit, ReviewedLightEdit::Absorb { .. }) {
            storage
                .save_light_profiles(&crate::storage::StoredLightProfiles::from_state(
                    &app.light_profile_configs,
                    &app.runtime_config,
                ))
                .map_err(|_| uncertain())?;
        }
        storage
            .save_rooms(&rooms_from_engine(ctx.runtime.as_ref()))
            .map_err(|_| uncertain())?;
    }
    let readback_verified = match &plan.edit {
        ReviewedLightEdit::RoomOverride { room_id, change } => ctx
            .runtime
            .engine_node_snapshot(room_id)
            .and_then(|n| {
                n.profile_settings
                    .profile_overrides
                    .get(&plan.base.id)
                    .cloned()
            })
            .is_some_and(|v| {
                change
                    .min_brightness
                    .is_none_or(|b| v.min_brightness == Some(b))
                    && change
                        .curve
                        .as_ref()
                        .is_none_or(|c| v.curve.as_ref() == Some(c))
            }),
        ReviewedLightEdit::Absorb { config } => {
            app.light_profile_config(&plan.base.id) == Some(config)
                && ctx
                    .runtime
                    .engine_all_node_snapshots()
                    .iter()
                    .filter(|n| n.kind.is_light_addressable() && n.parent_id.is_none())
                    .all(|n| n.time_offset_minutes == 0.)
        }
    };
    app.emit_event(crate::server_event::ServerEvent::ConfigChanged);
    app.emit_event(crate::server_event::ServerEvent::NodesChanged);
    drop(app);
    if !readback_verified {
        return Err(uncertain());
    }
    let mut record = crate::activity::LightActivityRecord::app(&plan.room_id, id);
    record.correlation_id = Some(plan.id.clone());
    crate::activity::record_light_activity(state, record);
    Ok(
        json!({"status":"applied","operation":id,"plan_id":token,"server_instance_id":plan.server,"contract_sha256":plan.contract,"server_acknowledged":true,"canonical_readback_verified":true,"physical_verification":"not_observed","profile_id":plan.base.id}),
    )
}

fn preview_light_output(
    state: &SharedState,
    id: &str,
    room: &NodeStateDto,
    args: &Value,
) -> Result<String> {
    if id == "preview_room_scene_draft" {
        let scene: crate::scenes::SceneDefinition =
            serde_json::from_value(args["scene"].clone())
                .map_err(|_| invalid("Expected a Rhythm scene definition."))?;
        if scene.light.is_none() || crate::scenes::is_native_scene_id(&scene.id) {
            return Err(invalid("Select an editable light scene."));
        }
        return commands::do_scene_draft_preview(
            state,
            SceneDraftPreviewRequest {
                target_id: room.id.clone(),
                scene,
                transition_ms: None,
                duration_ms: Some(60_000),
            },
        )
        .map_err(|_| uncertain());
    }
    if matches!(id, "apply_room_scene" | "preview_room_scene") {
        let scene_id = string(args, "scene_id")?;
        return commands::do_scene_preview(
            state,
            &scene_id,
            ScenePreviewRequest {
                target_id: room.id.clone(),
                transition_ms: None,
                duration_ms: Some(if id == "apply_room_scene" {
                    300_000
                } else {
                    60_000
                }),
            },
        )
        .map_err(|_| uncertain());
    }

    let (color, brightness) = if id == "set_room_color" {
        let color = crate::scenes::LightSceneColor::Rgb {
            rgb: rhythm_core::Rgb::new(
                int(args, "red", 0, 255)? as u8,
                int(args, "green", 0, 255)? as u8,
                int(args, "blue", 0, 255)? as u8,
            ),
        };
        (color, room.brightness)
    } else {
        guard_adaptive(room)?;
        let offset = num(args, "offset_minutes", -720., 720.)?;
        let app = lock(state)?;
        let ctx = context(&app, &room.id, room.state)?;
        let direct = matches!(
            &ctx.config.curve,
            LightCurveShape::Palette { .. }
                | LightCurveShape::SuperGaussian {
                    direct_color: Some(_),
                    ..
                }
                | LightCurveShape::Constant {
                    direct_color: Some(_),
                    ..
                }
        );
        let values = LightProfile::new(ctx.config).calculate_with_offset(&ctx.clock, offset);
        let color = if direct {
            crate::scenes::LightSceneColor::Rgb { rgb: values.rgb }
        } else {
            crate::scenes::LightSceneColor::Kelvin {
                kelvin: values.kelvin,
            }
        };
        (
            color,
            (f32::from(values.brightness) + ctx.room.brightness_offset).clamp(1., 100.) as u8,
        )
    };

    let request = SceneDraftPreviewRequest {
        target_id: room.id.clone(),
        transition_ms: None,
        duration_ms: Some(60_000),
        scene: crate::scenes::SceneDefinition {
            id: format!("trial-{}", uuid::Uuid::new_v4()),
            name: "Room light trial".into(),
            description: None,
            source: crate::scenes::SceneSource::User,
            extensions: Default::default(),
            light: Some(crate::scenes::LightSceneLayer {
                default_transition_ms: None,
                default_output: Some(crate::scenes::LightSceneOutput {
                    power: crate::scenes::LightScenePower::On,
                    color: Some(color),
                    brightness,
                    transition_ms: None,
                }),
                palette: Vec::new(),
                palette_mode: Default::default(),
                palette_seed: 0,
                entries: Vec::new(),
            }),
        },
    };
    commands::do_scene_draft_preview(state, request).map_err(|_| uncertain())
}

fn run(state: &SharedState, id: &str, args: &Value) -> Result<Value> {
    // Use the advertised schema for the field allowlist even when called over
    // HTTP directly. Values are checked again before passing to core commands.
    let descriptor = super::operation_registry()
        .iter()
        .find(|op| op.id == id)
        .ok_or_else(|| invalid("Unknown lighting operation."))?;
    let fields = args
        .as_object()
        .ok_or_else(|| invalid("Expected an arguments object."))?;
    if fields
        .keys()
        .any(|k| descriptor.input_schema["properties"].get(k).is_none())
    {
        return Err(invalid("Unrecognized lighting argument."));
    }
    if matches!(id, "apply_room_tuning" | "apply_absorb_offset") {
        return apply(state, id, args);
    }
    if matches!(id, "run_mode_transition" | "run_schedule_transition") {
        let transition = string(args, "transition_id")?;
        let result = if id == "run_mode_transition" {
            commands::do_trigger_transition(state, &transition).map_err(|_| uncertain())?;
            json!({"scope":"hub","mode":lock(state)?.active_mode})
        } else {
            let schedule = string(args, "schedule_id")?;
            let outcome =
                commands::do_trigger_light_schedule_transition(state, &schedule, &transition)
                    .map_err(|_| uncertain())?;
            json!({"scope":"schedule","schedule_id":schedule,"mode":outcome.target_mode})
        };
        return Ok(
            json!({"result":result,"transition_id":transition,"server_acknowledged":true,"physical_verification":"not_observed"}),
        );
    }
    if id == "set_house_lighting_mode" {
        let mode = match enum_arg(args, "mode", &["day", "sleep"])? {
            "day" => RhythmMode::Day,
            _ => RhythmMode::Sleep,
        };
        commands::do_set_active_mode(state, mode).map_err(|_| uncertain())?;
        let app = lock(state)?;
        return Ok(
            json!({"scope":"hub","mode":app.active_mode,"canonical_readback_verified":app.active_mode==mode,"physical_verification":"not_observed"}),
        );
    }
    let room_id = string(args, "room_id")?;
    if lock(state)?.topology.get(&room_id).is_none() {
        return Err(invalid("Select a discovered Rhythm room."));
    }
    if descriptor.tier != OperationTier::Read
        && !lock(state)?.rhythm_automation_allowed_for_node(&room_id)
    {
        return Err(unavailable(
            "Review ownership in the controlling integration before operating this room.",
        ));
    }
    let room = commands::build_node_state(state, &room_id)
        .map_err(|_| unavailable("Room unavailable in the lighting engine."))?;
    if id == "explain_room_output" {
        return describe(state, &room, None);
    }
    if id == "read_room_lighting_setup" {
        return setup(state, &room);
    }
    if id == "plan_room_tuning" || id == "plan_absorb_offset" {
        return plan(state, &room, id, args);
    }
    if id == "sample_room_curve" {
        let hour = num(args, "hour", 0., 23.999)?;
        let app = lock(state)?;
        let ctx = context(&app, &room_id, room.state)?;
        let registry = commands::light_profile_registry_from_state(&app);
        let clock = CurveContext::new(hour, ctx.clock.solar, ctx.clock.sun_times);
        let values = registry.calculate_room_values(
            ctx.mode,
            room.state,
            Some(&ctx.room.profile_settings),
            &clock,
            ctx.room.time_offset_minutes,
        );
        return Ok(
            json!({"kind":"profile_sample","room_id":room_id,"profile_id":ctx.config.id,"hour":hour,"brightness":(f32::from(values.brightness)+ctx.room.brightness_offset).clamp(1.,100.) as u8,"kelvin":values.kelvin,"rgb":values.rgb,"current_room_state":room.state}),
        );
    }
    let since = current_epoch_ms();
    let mut trial = None;
    let mut unresolved = Vec::new();
    match id {
        "control_room_power" | "step_room_curve" | "return_room_to_schedule" => {
            let action = match id {
                "control_room_power" => {
                    match enum_arg(args, "power", &["on", "off", "hard_off"])? {
                        "hard_off" => "lights_off",
                        s => s,
                    }
                }
                "step_room_curve" => {
                    guard_adaptive(&room)?;
                    match enum_arg(args, "direction", &["up", "down"])? {
                        "up" => "step_up",
                        _ => "step_down",
                    }
                }
                _ => match enum_arg(
                    args,
                    "behavior",
                    &["resume", "reset_offsets", "scheduled_default"],
                )? {
                    "resume" => "rhythm_on",
                    "reset_offsets" => "reset",
                    _ => "reset_to_mode_default",
                },
            };
            commands::do_node_action(state, &room_id, action, false).map_err(|_| uncertain())?;
        }
        "adjust_room_brightness" | "adjust_room_warmth" => {
            guard_adaptive(&room)?;
            let modifier = if id == "adjust_room_brightness" {
                let delta = int(args, "delta_percent", -100, 100)?;
                if delta == 0 {
                    return Err(invalid("Choose a nonzero brightness change."));
                }
                commands::NodeCurveModifier::Brightness(
                    (i64::from(room.brightness) + delta).clamp(1, 100) as u8,
                )
            } else {
                commands::NodeCurveModifier::ColorTemperature {
                    kelvin: int(args, "kelvin", 500, 25000)? as u16,
                    preserve_brightness: true,
                }
            };
            commands::do_set_node_curve_modifier(state, &room_id, modifier, false)
                .map_err(|_| uncertain())?;
        }
        "try_room_offset"
        | "set_room_color"
        | "preview_room_scene_draft"
        | "preview_room_scene"
        | "apply_room_scene" => {
            let response = preview_light_output(state, id, &room, args)?;
            let response: SceneApplyResponse =
                serde_json::from_str(&response).map_err(|_| uncertain())?;
            trial = response.preview_id;
            unresolved = response.unresolved_node_ids;
        }
        "set_room_offset" => {
            guard_adaptive(&room)?;
            if lock(state)?.scene_preview_blocks_node(&room_id, since) {
                return Err(invalid(
                    "End the current trial before keeping an adaptive offset.",
                ));
            }
            let offset = num(args, "offset_minutes", -720., 720.)?;
            commands::do_set_node_time_offset(state, &room_id, offset, false)
                .map_err(|_| uncertain())?;
        }
        "undo_room_trial" => {
            let token = string(args, "trial_id")?;
            if !lock(state)?
                .light_scene_previews
                .get(&token)
                .is_some_and(|p| p.target_node_id == room_id)
            {
                return Err(stale());
            }
            commands::do_scene_preview_cancel(state, &token).map_err(|_| uncertain())?;
        }
        _ => return Err(invalid("Unknown lighting operation.")),
    }
    let updated = commands::build_node_state(state, &room_id).map_err(|_| uncertain())?;
    let mut response = describe(state, &updated, Some(since))?;
    response["server_acknowledged"] = json!(true);
    response["physical_verification"] = json!("see_per_light_readback");
    response["unresolved_node_ids"] = json!(unresolved);
    if let Some(token) = trial {
        response["expires_at_epoch_ms"] = json!(lock(state)?
            .light_scene_previews
            .get(&token)
            .map(|p| p.expires_at_epoch_ms));
        response["trial_id"] = json!(token);
    }
    Ok(response)
}

pub async fn call(state: &SharedState, id: &str, arguments: Value) -> Result<Value> {
    run(state, id, &arguments)
}
