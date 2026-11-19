//! Opt-in, in-memory walk tests. Only a positive report from the selected
//! canonical sensor can advance a test. No household event history is retained.
use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use rhythm_core::runtime::hub_registry::DeviceType;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use super::{
    object_schema, operation, validated_correlation_id, LightAssistantError as Error,
    LightAssistantOperationDto, OperationTier,
};
use crate::state::{current_epoch_ms, AppState, SharedState};
use crate::topology::NodeControlKind;

const WALK_WINDOW: Duration = Duration::from_secs(60);
const RETENTION: Duration = Duration::from_secs(300);
const LIMIT: usize = 32;

#[derive(Default)]
pub struct MotionSessions {
    next_detection_id: u64,
    walks: HashMap<String, Walk>,
    plans: HashMap<String, Plan>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Target {
    sensor_id: String,
    room_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WalkStep {
    Routed,
    MissingTarget,
    Suppressed(&'static str),
    NotNeeded(&'static str),
    Queued,
    Dispatching,
    Acknowledged,
    NoDispatch,
    Uncertain,
}

impl WalkStep {
    fn describe(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Routed => ("routed", "evaluating_policy", "Inspect this walk again."),
            Self::MissingTarget => ("detected", "missing_target", "Review a motion target plan for this room."),
            Self::Suppressed(reason) => ("suppressed", reason, "Inspect the room's motion policy, scene and controller ownership."),
            Self::NotNeeded(reason) => ("command_not_needed", reason, "Check the individual lights and ask the resident what they see."),
            Self::Queued => ("command_queued", "awaiting_dispatch", "Inspect this walk again for a command acknowledgement."),
            Self::Dispatching => ("command_attempted", "awaiting_acknowledgement", "Inspect this walk again for acknowledgement and light readback."),
            Self::Acknowledged => ("command_acknowledged", "check_each_light", "Check each light's fresh device readback. Ask the resident to verify any uncertain light or use Identify."),
            Self::NoDispatch => ("no_command_issued", "no_dispatch_route", "Check whether this room has a reachable managed light."),
            Self::Uncertain => ("command_uncertain", "dispatch_failed_or_partial", "Inspect light readback and integration availability before retrying."),
        }
    }
}

struct Detection {
    event: u64,
    epoch_ms: u64,
    routed_targets: Vec<String>,
    step: WalkStep,
    dispatch_epoch_ms: Option<u64>,
}

struct Walk {
    started: Instant,
    server_instance_id: String,
    target: Target,
    detection: Option<Detection>,
    lights: BTreeMap<String, WalkLight>,
}

#[derive(Serialize)]
struct WalkLight {
    availability: &'static str,
    readback: &'static str,
    power_on: Option<bool>,
    observed_at_epoch_ms: Option<u64>,
}

#[derive(Clone)]
struct Plan {
    created: Instant,
    server_instance_id: String,
    target: Target,
    correlation_id: String,
    topology_sha256: String,
    after_targets: Vec<String>,
    applied_at: Option<u64>,
}

pub(super) fn operations() -> Vec<LightAssistantOperationDto> {
    let sensor = json!({"type":"string","description":"Canonical motion sensor ID. An undiscovered sensor returns a setup handoff."});
    let room = json!({"type":"string","description":"Intended Rhythm room ID from topology."});
    let target = object_schema(
        &["sensor_id", "room_id"],
        json!({"sensor_id":sensor,"room_id":room}),
    );
    let walk = object_schema(
        &["walk_id"],
        json!({
            "walk_id":{"type":"string"},
            "resident_observations":{
                "type":"array",
                "description":"Only include lights the resident personally verified during this walk. These reports are distinct from fresh device readback.",
                "items":object_schema(&["device_id","power_on"], json!({
                    "device_id":{"type":"string"}, "power_on":{"type":"boolean"}
                }))
            }
        }),
    );
    let plan = object_schema(
        &["sensor_id", "room_id", "correlation_id"],
        json!({"sensor_id":sensor,"room_id":room,"correlation_id":{"type":"string"}}),
    );
    let apply = object_schema(
        &["plan_id", "server_instance_id", "confirmed"],
        json!({"plan_id":{"type":"string"},"server_instance_id":{"type":"string"},"confirmed":{"type":"boolean","description":"True only after the person reviews the before/after targets and explicitly approves."}}),
    );
    let result = json!({"type":"object","required":["status","server_instance_id"],"properties":{"status":{"type":"string"},"server_instance_id":{"type":"string"}}});
    vec![
        operation("start_motion_walk","Ask the resident to walk past the selected sensor. Start a fresh 60-second test for the intended room. The response includes the physical room, targets and integration, or the physical setup step needed to discover the sensor.",OperationTier::Read,target,result.clone()),
        operation("inspect_motion_walk","Inspect the first fresh detection in this walk, its actual routing, room policy, dispatch and per-light reports. Acknowledgement alone does not establish the light came on. Ask the resident about missing readback and distinguish their visual confirmation from device reports.",OperationTier::Read,walk,result.clone()),
        operation("plan_motion_target","Review adding this room to a motion sensor's control targets. Physical placement and all current effective targets, including an inherited room, are preserved.",OperationTier::Read,plan,result.clone()),
        operation("apply_motion_target","Apply a reviewed sensor link after explicit confirmation. Reread the targets, then start a new walk for the same sensor and room. Refresh and plan again after a conflict.",OperationTier::PersistentChange,apply,result),
    ]
}

fn parse<T: DeserializeOwned>(arguments: Value) -> Result<T, Error> {
    serde_json::from_value(arguments).map_err(|_| {
        Error::bad_request(
            "invalid_arguments",
            "Supply the arguments declared by this motion tool.",
        )
    })
}

fn unavailable() -> Error {
    Error::internal("state_unavailable", "Rhythm state is unavailable.")
}

pub(crate) async fn call(state: &SharedState, id: &str, arguments: Value) -> Result<Value, Error> {
    match id {
        "start_motion_walk" => start(state, parse(arguments)?),
        "inspect_motion_walk" => inspect(state, parse(arguments)?),
        "plan_motion_target" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Preview {
                sensor_id: String,
                room_id: String,
                correlation_id: String,
            }
            let req: Preview = parse(arguments)?;
            preview(
                state,
                Target {
                    sensor_id: req.sensor_id,
                    room_id: req.room_id,
                },
                validated_correlation_id(&req.correlation_id)?,
            )
        }
        "apply_motion_target" => apply(state, parse(arguments)?),
        _ => Err(Error::bad_request(
            "unknown_operation",
            "Unknown motion operation.",
        )),
    }
}

fn handoff(s: &AppState, target: &Target, authority: bool) -> Value {
    let (surface, instruction) = match (authority,s.platform_context) {
        (false,"ha_addon") => ("home_assistant", "Add the sensor to its Home Assistant integration and check device discovery again on this hub."),
        (false,_) => ("rhythm_app", "Open Add Device on this hub in Rhythm. Pair the sensor by phone or add it to its bridge, then check discovery again here."),
        (true,"ha_addon") => ("home_assistant", "Review ownership of the intended room and its devices in Home Assistant before changing motion routing."),
        (true,_) => ("integration", "Review room automation ownership in the controlling integration before changing motion routing."),
    };
    json!({
        "status":"handoff","server_instance_id":s.server_instance_id,
        "sensor_id":target.sensor_id,"room_id":target.room_id,
        "handoff":{"actor":"resident","surface":surface,"instruction":instruction,"resume_operation":"start_motion_walk"}
    })
}

fn check_target(s: &AppState, t: &Target, require_authority: bool) -> Result<Option<Value>, Error> {
    if s.topology.get(&t.room_id).is_none() {
        return Err(Error::bad_request(
            "unknown_room",
            "Select a Rhythm room from the topology.",
        ));
    }
    let Some(sensor) = s
        .canonical_registry
        .get(&t.sensor_id)
        .filter(|d| !d.is_removed() && s.topology.has_public_node(&d.id))
    else {
        return Ok(Some(handoff(s, t, false)));
    };
    if sensor.device_type != DeviceType::Motion {
        return Err(Error::bad_request(
            "not_motion_sensor",
            "Select a discovered motion sensor.",
        ));
    }
    if require_authority
        && (!s.rhythm_automation_allowed_for_node(&t.sensor_id)
            || !s.rhythm_automation_allowed_for_node(&t.room_id))
    {
        return Ok(Some(handoff(s, t, true)));
    }
    Ok(None)
}

fn targets(s: &AppState, id: &str) -> Vec<String> {
    s.topology
        .effective_control_targets(id, &NodeControlKind::Motion)
}

fn context(s: &AppState, t: &Target) -> Value {
    let device = s.canonical_registry.get(&t.sensor_id);
    let integrations: std::collections::BTreeSet<_> = device
        .into_iter()
        .flat_map(|d| d.active_endpoints().map(|ep| ep.hub_key.hub_type.as_str()))
        .collect();
    json!({
        "server_instance_id":s.server_instance_id,
        "sensor_id":t.sensor_id, "room_id":t.room_id,
        "physical_room_id":s.topology.device_parent_room_id(&t.sensor_id),
        "integrations":integrations, "control_targets":targets(s,&t.sensor_id)
    })
}

fn busy() -> Error {
    Error {
        status: 429,
        code: "motion_session_limit",
        message: "Active motion sessions are at capacity. Retry after an existing session expires."
            .into(),
        retryable: true,
        mutation_may_have_applied: false,
    }
}

fn start(state: &SharedState, t: Target) -> Result<Value, Error> {
    let mut s = state.lock().map_err(|_| unavailable())?;
    if let Some(handoff) = check_target(&s, &t, false)? {
        return Ok(handoff);
    }
    s.assistant_motion
        .walks
        .retain(|_, w| w.started.elapsed() < RETENTION);
    if s.assistant_motion.walks.len() >= LIMIT {
        return Err(busy());
    }
    let mut result = context(&s, &t);
    let lights = s
        .topology
        .get(&t.room_id)
        .into_iter()
        .flat_map(|r| &r.devices)
        .filter_map(|d| s.canonical_registry.get(&d.device_id))
        .filter(|d| !d.is_removed() && d.device_type == DeviceType::Light)
        .map(|d| {
            let offline = d
                .active_endpoints()
                .all(|ep| s.hub_connection_status.get(&ep.hub_key) == Some(&false));
            let reported_offline = s
                .light_observations
                .get(&d.id)
                .is_some_and(|o| o.availability == "unavailable");
            (
                d.id.clone(),
                WalkLight {
                    availability: if offline || reported_offline {
                        "unavailable"
                    } else {
                        "unknown"
                    },
                    readback: "no_fresh_readback",
                    power_on: None,
                    observed_at_epoch_ms: None,
                },
            )
        })
        .collect();
    let walk_id = Uuid::new_v4().to_string();
    result["status"] = json!("walk_started");
    result["walk_id"] = json!(walk_id);
    result["window_seconds"] = json!(WALK_WINDOW.as_secs());
    result["next_check"] = json!("Walk past this sensor now, then inspect the walk.");
    let server_instance_id = s.server_instance_id.clone();
    s.assistant_motion.walks.insert(
        walk_id.clone(),
        Walk {
            started: Instant::now(),
            server_instance_id,
            target: t,
            detection: None,
            lights,
        },
    );
    // The selected household activity is discarded even if the client never
    // polls again. The weak reference does not extend the hub lifetime.
    let hub = std::sync::Arc::downgrade(state);
    tokio::spawn(async move {
        tokio::time::sleep(RETENTION).await;
        if let Some(state) = hub.upgrade() {
            if let Ok(mut app) = state.lock() {
                app.assistant_motion.walks.remove(&walk_id);
            }
        }
    });
    Ok(result)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inspect {
    walk_id: String,
    #[serde(default)]
    resident_observations: Vec<ResidentObservation>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResidentObservation {
    device_id: String,
    power_on: bool,
}

fn inspect(state: &SharedState, request: Inspect) -> Result<Value, Error> {
    let mut s = state.lock().map_err(|_| unavailable())?;
    s.assistant_motion
        .walks
        .retain(|_, w| w.started.elapsed() < RETENTION);
    let walk = s
        .assistant_motion
        .walks
        .get(&request.walk_id)
        .filter(|w| w.server_instance_id == s.server_instance_id)
        .ok_or_else(|| Error::conflict("walk_expired", "Start a new walk on the same hub."))?;
    let active = walk.started.elapsed() < WALK_WINDOW;
    let (stage, reason, next) = walk
        .detection
        .as_ref()
        .map(|d| d.step.describe())
        .unwrap_or((
            "waiting_for_detection",
            "no_fresh_detection",
            if active {
                "Walk past the selected sensor and inspect again."
            } else {
                "Check the sensor battery, discovery and integration, then start a new walk."
            },
        ));
    let mut confirmed = BTreeMap::new();
    for observation in &request.resident_observations {
        if !walk.lights.contains_key(&observation.device_id)
            || confirmed
                .insert(&observation.device_id, observation.power_on)
                .is_some()
        {
            return Err(Error::bad_request(
                "invalid_resident_observation",
                "Supply at most one observation for each light in this walk.",
            ));
        }
    }
    if !confirmed.is_empty()
        && !walk.detection.as_ref().is_some_and(|d| {
            d.dispatch_epoch_ms.is_some() || matches!(d.step, WalkStep::NotNeeded(_))
        })
    {
        return Err(Error::bad_request(
            "walk_not_routed",
            "Wait until motion reaches the room before reporting the visual result.",
        ));
    }
    let mut result = context(&s, &walk.target);
    let lights: Vec<_> = walk
        .lights
        .iter()
        .map(|(id, light)| {
            let mut item = json!(light);
            item["device_id"] = json!(id);
            item["resident_power_on"] = json!(confirmed.get(id));
            item
        })
        .collect();
    result["status"] = json!(if active {
        "walk_active"
    } else {
        "walk_finished"
    });
    result["walk_id"] = json!(request.walk_id);
    result["stage"] = json!(stage);
    result["reason"] = json!(reason);
    result["next_check"] = json!(next);
    result["detected_at_epoch_ms"] = json!(walk.detection.as_ref().map(|d| d.epoch_ms));
    result["routed_targets"] = json!(walk.detection.as_ref().map(|d| &d.routed_targets));
    result["lights"] = json!(lights);
    result["resident_confirmation"] = json!(if confirmed.is_empty() {
        "not_recorded"
    } else {
        "reported"
    });
    Ok(result)
}

/// Match the first positive report after the user started each active walk.
/// The event ID binds subsequent policy/worker updates to this exact event.
pub(crate) fn record_detection(
    state: &SharedState,
    sensor: &str,
    detected: bool,
    routed: &[String],
    suppressed: bool,
) -> Option<u64> {
    if !detected {
        return None;
    }
    let mut s = state.lock().ok()?;
    if s.assistant_motion.walks.is_empty() {
        return None;
    }
    s.assistant_motion.next_detection_id = s.assistant_motion.next_detection_id.wrapping_add(1);
    let event = s.assistant_motion.next_detection_id;
    let mut recorded = false;
    for w in s.assistant_motion.walks.values_mut().filter(|w| {
        w.target.sensor_id == sensor && w.started.elapsed() < WALK_WINDOW && w.detection.is_none()
    }) {
        let step = if suppressed {
            WalkStep::Suppressed("external_room_authority")
        } else if routed.contains(&w.target.room_id) {
            WalkStep::Routed
        } else {
            WalkStep::MissingTarget
        };
        w.detection = Some(Detection {
            event,
            epoch_ms: current_epoch_ms(),
            routed_targets: routed.to_vec(),
            step,
            dispatch_epoch_ms: None,
        });
        recorded = true;
    }
    recorded.then_some(event)
}

pub(crate) fn record_step(state: &SharedState, event: Option<u64>, room: &str, step: WalkStep) {
    let Some(event) = event else { return };
    let Ok(mut s) = state.lock() else { return };
    for walk in s
        .assistant_motion
        .walks
        .values_mut()
        .filter(|w| w.target.room_id == room && w.started.elapsed() < WALK_WINDOW)
    {
        if let Some(d) = walk.detection.as_mut().filter(|d| d.event == event) {
            if step == WalkStep::Dispatching {
                d.dispatch_epoch_ms = Some(current_epoch_ms());
            }
            d.step = step;
        }
    }
}

/// Only integration reports reach this hook. Command-derived cached power,
/// grouped room observations and pre-command HA inventory cannot verify a bulb.
pub(crate) fn record_light_report(
    s: &mut AppState,
    light: &str,
    available: bool,
    power: Option<bool>,
    source_ms: u64,
) {
    let parent = s.topology.device_parent_room_id(light);
    for w in
        s.assistant_motion.walks.values_mut().filter(|w| {
            w.started.elapsed() < WALK_WINDOW && parent == Some(w.target.room_id.as_str())
        })
    {
        let Some(d) = &w.detection else { continue };
        let Some(item) = w.lights.get_mut(light) else {
            continue;
        };
        let since = match d.step {
            WalkStep::Dispatching | WalkStep::Acknowledged | WalkStep::Uncertain => {
                d.dispatch_epoch_ms
            }
            WalkStep::NotNeeded(_) => Some(d.epoch_ms),
            _ => None,
        };
        if !since.is_some_and(|ms| source_ms >= ms)
            || item
                .observed_at_epoch_ms
                .is_some_and(|last| source_ms < last)
        {
            continue;
        }
        item.availability = if available {
            "available"
        } else {
            "unavailable"
        };
        item.power_on = if available { power } else { None };
        item.readback = if item.power_on.is_some() {
            "fresh_device_readback"
        } else {
            "no_fresh_readback"
        };
        item.observed_at_epoch_ms = Some(source_ms);
    }
}

fn preview(state: &SharedState, t: Target, correlation_id: String) -> Result<Value, Error> {
    let mut s = state.lock().map_err(|_| unavailable())?;
    if let Some(handoff) = check_target(&s, &t, true)? {
        return Ok(handoff);
    }
    let before = targets(&s, &t.sensor_id);
    let mut result = context(&s, &t);
    result["before_targets"] = json!(before);
    result["correlation_id"] = json!(correlation_id);
    if before.contains(&t.room_id) {
        result["status"] = json!("already_linked");
        result["after_targets"] = json!(before);
        return Ok(result);
    }
    s.assistant_motion
        .plans
        .retain(|_, p| p.created.elapsed() < RETENTION);
    if s.assistant_motion.plans.len() >= LIMIT {
        return Err(busy());
    }
    let topology_sha256 =
        crate::commands::topology_resource_sha256_from_app_state(&s).map_err(|_| unavailable())?;
    let mut after = before;
    after.push(t.room_id.clone());
    after.sort();
    let id = Uuid::new_v4().to_string();
    result["status"] = json!("ready");
    result["plan_id"] = json!(id);
    result["topology_resource_sha256"] = json!(topology_sha256);
    result["after_targets"] = json!(after);
    result["requires_confirmation"] = json!(true);
    let server_instance_id = s.server_instance_id.clone();
    s.assistant_motion.plans.insert(
        id,
        Plan {
            created: Instant::now(),
            server_instance_id,
            target: t,
            correlation_id,
            topology_sha256,
            after_targets: after,
            applied_at: None,
        },
    );
    Ok(result)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Apply {
    plan_id: String,
    server_instance_id: String,
    confirmed: bool,
}

fn apply(state: &SharedState, req: Apply) -> Result<Value, Error> {
    if !req.confirmed {
        return Err(Error::bad_request(
            "confirmation_required",
            "Explicit confirmation of the reviewed link is required.",
        ));
    }
    let transaction = state
        .lock()
        .map_err(|_| unavailable())?
        .external_topology_transaction_lock
        .clone();
    let _guard = transaction.lock().map_err(|_| unavailable())?;
    let mut s = state.lock().map_err(|_| unavailable())?;
    if s.server_instance_id != req.server_instance_id {
        return Err(Error::conflict(
            "hub_changed",
            "Refresh the selected hub and preview again.",
        ));
    }
    let plan = s
        .assistant_motion
        .plans
        .get(&req.plan_id)
        .filter(|p| p.created.elapsed() < RETENTION && p.server_instance_id == s.server_instance_id)
        .cloned()
        .ok_or_else(|| Error::conflict("plan_expired", "Refresh the sensor and preview again."))?;
    if let Some(handoff) = check_target(&s, &plan.target, true)? {
        return Ok(handoff);
    }
    let hash =
        crate::commands::topology_resource_sha256_from_app_state(&s).map_err(|_| unavailable())?;
    let applied_at = match plan.applied_at {
        Some(applied_at) => applied_at,
        None => {
            if hash != plan.topology_sha256 {
                return Err(Error::conflict(
                    "topology_changed",
                    "Refresh the sensor and review a new plan.",
                ));
            }
            crate::commands::set_topology_control_targets_locked(
                &mut s,
                &plan.target.sensor_id,
                NodeControlKind::Motion,
                &plan.after_targets,
            )
            .map_err(|_| {
                Error::internal("link_failed", "Could not save the reviewed sensor link.")
            })?;
            let at = current_epoch_ms();
            s.assistant_motion
                .plans
                .get_mut(&req.plan_id)
                .unwrap()
                .applied_at = Some(at);
            at
        }
    };
    if targets(&s, &plan.target.sensor_id) != plan.after_targets {
        return Err(Error::conflict(
            "targets_changed",
            "Reread the sensor's current links before making another change.",
        ));
    }
    let mut result = context(&s, &plan.target);
    result["status"] = json!("applied");
    result["correlation_id"] = json!(plan.correlation_id);
    result["plan_id"] = json!(req.plan_id);
    result["canonical_readback_verified"] = json!(true);
    result["completed_at_epoch_ms"] = json!(applied_at);
    result["next_check"] = json!("Start a new motion walk for the same sensor and room.");
    drop(s);
    drop(_guard);
    if plan.applied_at.is_none() {
        let mut activity = crate::activity::LightActivityRecord::app(
            &plan.target.sensor_id,
            "assistant_link_motion_target",
        );
        activity.correlation_id = Some(plan.correlation_id);
        activity.payload = Some(json!({"status":"applied","canonical_readback_verified":true}));
        crate::activity::record_light_activity(state, activity);
    }
    Ok(result)
}
