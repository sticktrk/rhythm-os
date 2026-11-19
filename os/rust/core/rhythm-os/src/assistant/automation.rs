//! Reviewed edits to the lighting automations owned by this installation.
use super::{
    object_schema, operation, LightAssistantError as Error, LightAssistantOperationDto,
    OperationTier,
};
use crate::commands::{self, ReviewCheck};
use crate::scenes::{HomeSceneApplyResponse, SceneDefinition};
use crate::state::{AppState, SharedState, UpdateChannel};
use crate::topology::InputBinding;
use rhythm_core::{
    LightScheduleConfig, LightScheduleOverride, ModeConfig, ModeTransitionConfig, RoomModeState,
};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, Error>;
fn invalid(text: impl Into<String>) -> Error {
    Error::bad_request("invalid_automation", text)
}
fn stale() -> Error {
    Error::conflict(
        "automation_changed",
        "The reviewed configuration changed. Read it and preview again.",
    )
}
fn parse<T: DeserializeOwned>(value: &Value) -> Result<T> {
    serde_json::from_value(value.clone()).map_err(|e| invalid(format!("Invalid setting: {e}")))
}
fn str_arg(args: &Value, key: &str) -> Result<String> {
    args[key]
        .as_str()
        .filter(|v| !v.trim().is_empty() && v.len() <= 256)
        .map(str::to_owned)
        .ok_or_else(|| invalid(format!("Choose a valid {key}.")))
}
fn text_schema() -> Value {
    json!({"type":"string","minLength":1,"maxLength":256})
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum Edit {
    KeepScene {
        room_id: String,
        preview_id: String,
        scene_id: String,
    },
    HomeScene {
        id: String,
        target_mode: crate::scenes::HomeSceneTargetMode,
    },
    Scene {
        id: String,
        definition: Option<SceneDefinition>,
    },
    Schedule {
        id: String,
        definition: Option<LightScheduleConfig>,
    },
    Binding {
        id: String,
        binding: Option<InputBinding>,
    },
    Transitions(Vec<ModeTransitionConfig>),
    Modes(Vec<ModeConfig>),
    Assignment {
        node: String,
        authority: String,
        schedule: Option<String>,
    },
    ScheduleOverride {
        node: String,
        schedule: String,
        value: Option<LightScheduleOverride>,
    },
    Preferences {
        node: String,
        rhythm: Option<bool>,
        disabled: Option<bool>,
        standby: Option<bool>,
        mode: Option<RoomModeState>,
        profile: Option<Value>,
    },
    Breaker(bool),
    Runtime(String),
    Updates {
        auto: bool,
        channel: UpdateChannel,
    },
}

pub struct AutomationPlan {
    token: String,
    operation: String,
    server: String,
    valid_until: Instant,
    before: Value,
    edit: Edit,
}

fn mutation_specs() -> Vec<(&'static str, &'static str, Value)> {
    let text = text_schema();
    let nullable = |props: Value| json!({"anyOf":[props,{"type":"null"}]});
    let def = |fields: &[&str], props: Value| object_schema(fields, props);
    let scene_schema = def(
        &["id", "name", "light"],
        json!({"id":text,"name":text,"description":{"type":["string","null"]},"source":{"type":"object"},"light":{"type":"object","description":"A Rhythm light scene layer containing default_output, palette or entries. Read a saved scene for its full definition."},"extensions":{"type":"object"}}),
    );
    let schedule_schema = def(
        &["id", "name", "transitions"],
        json!({"id":text,"name":text,"enabled":{"type":"boolean"},"active_mode":{"enum":["day","sleep"]},"transitions":{"type":"array","maxItems":128,"items":{"type":"object"}}}),
    );
    let binding_schema = def(
        &["id", "source_node_id", "trigger", "action", "enabled"],
        json!({"id":text,"source_node_id":text,"preset":{"enum":["day_sleep_toggle",null]},"trigger":{"type":"object"},"action":{"type":"object"},"enabled":{"type":"boolean"}}),
    );
    vec![
        ("scene_preview_commit","Save a reviewed trial as the room's Mood scene. A draft saves its scene definition. Review the previous binding and any replaced scene first.",def(&["room_id","trial_id"],json!({"room_id":text,"trial_id":text}))),
        ("home_scene","Apply a saved scene across every eligible managed room or light. This replaces their saved Mood bindings and dispatches light commands. Review the previous bindings and the whole-home scope.",def(&["scene_id","target_mode"],json!({"scene_id":text,"target_mode":{"enum":["rooms","devices"]}}))),
        ("scene","Create or edit one saved scene. Supply its complete light layer. To delete it, supply null. Review rooms whose Mood binding will be removed and whose lights can turn on.",def(&["scene_id","definition"],json!({"scene_id":text,"definition":nullable(scene_schema)}))),
        ("light_schedule","Create or edit one named schedule, or delete it with a null definition. Review affected rooms. A schedule still in use cannot be deleted.",def(&["schedule_id","definition"],json!({"schedule_id":text,"definition":nullable(schedule_schema)}))),
        ("input_binding","Create or edit one button binding, or delete it with a null binding. Use discovered button IDs. The before/after state includes the trigger and its mode action.",def(&["binding_id","binding"],json!({"binding_id":text,"binding":nullable(binding_schema)}))),
        ("mode_transitions","Edit the hub's Day/Sleep transitions. Submit the full reviewed list, retaining each intended transition and its trigger enablement.",def(&["transitions"],json!({"transitions":{"type":"array","maxItems":128,"items":{"type":"object"}}}))),
        ("mode_settings","Edit Day/Sleep profile routing and per-room mode defaults. Review all affected rooms and changes to current light output.",def(&["modes"],json!({"modes":{"type":"array","maxItems":2,"items":{"type":"object"}}}))),
        ("schedule_assignment","Assign a room or standalone light to a named schedule, legacy global mode, or explicitly unscheduled operation. The new authority can change its power default immediately.",def(&["node_id","authority"],json!({"node_id":text,"authority":{"enum":["named","legacy","unscheduled"]},"schedule_id":text}))),
        ("schedule_override","Edit one node's named-schedule customization, or clear it with a null override. Other schedules are retained.",def(&["node_id","schedule_id","override"],json!({"node_id":text,"schedule_id":text,"override":nullable(json!({"type":"object"}))}))),
        ("node_preferences","Change the selected light or room's adaptation, disablement, Low Glow, motion activation or per-profile settings. Read the current values first. A new mode may change power immediately.",def(&["node_id"],json!({"node_id":text,"rhythm_enabled":{"type":"boolean"},"disabled":{"type":"boolean"},"standby_enabled":{"type":"boolean"},"state":{"enum":["active","mood","standby","hard_off"]},"profile_settings":{"type":"object","description":"Sparse room profile patch. For motion activation set motion_activation_enabled. For per-profile controls supply profile_overrides keyed by profile ID."}}))),
        ("light_breaker","Change the hub-wide autonomous light breaker. Review its current value and the affected rooms.",def(&["enabled"],json!({"enabled":{"type":"boolean"}}))),
        ("light_runtime","Select a lighting runtime advertised by this hub. Applies to all rooms.",def(&["runtime_id"],json!({"runtime_id":text}))),
        ("update_settings","Set automatic updates and select the stable or beta feed for a standalone installation. Home Assistant manages add-on updates.",def(&["auto_update","update_channel"],json!({"auto_update":{"type":"boolean"},"update_channel":{"enum":["stable","beta"]}}))),
    ]
}

pub(super) fn operations() -> Vec<LightAssistantOperationDto> {
    let mut out=vec![operation("read_automation_settings","Read saved scene definitions, schedules, transitions, bindings, operating modes and lighting settings before planning changes. Use read_lighting_state for individual node preferences.",OperationTier::Read,object_schema(&[],json!({})),json!({"type":"object"}))];
    for (kind, description, schema) in mutation_specs() {
        out.push(operation(
            &format!("plan_{kind}_edit"),
            description,
            OperationTier::Read,
            schema,
            json!({"type":"object","required":["plan_id","before","proposal","affected_rooms"]}),
        ));
        out.push(operation(
            &format!("apply_{kind}_edit"),
            &format!("Save the reviewed {kind} change after explicit confirmation."),
            OperationTier::PersistentChange,
            object_schema(
                &["plan_id", "confirmed"],
                json!({"plan_id":text_schema(),"confirmed":{"type":"boolean","const":true}}),
            ),
            json!({"type":"object","required":["plan_id","status","after"]}),
        ));
    }
    out
}

fn room_ids(app: &AppState) -> Vec<String> {
    let mut rooms: Vec<_> = app.topology.rooms().map(|r| r.id.clone()).collect();
    rooms.sort();
    rooms
}
fn settings(app: &AppState) -> Value {
    json!({
        "scenes":app.scenes.values().collect::<Vec<_>>(),
        "schedules":app.light_schedule_configs(),"transitions":app.mode_transition_configs(),
        "bindings":app.topology.input_bindings(),"modes":app.mode_configs(),
        "light_breaker_enabled":app.light_breaker_enabled,"runtime_id":app.light_runtime_kind.as_str(),
        "runtimes":app.light_runtime_registry.manifests(),"auto_update":app.auto_update,"update_channel":app.update_channel,
    })
}

impl Edit {
    fn parse(app: &AppState, kind: &str, args: &Value) -> Result<Self> {
        let invalid_model = |e: anyhow::Error| invalid(e.to_string());
        if args.to_string().len() > 128 * 1024 {
            return Err(invalid("This edit exceeds the maximum size."));
        }
        let edit = match kind {
            "scene_preview_commit" => {
                let preview_id = str_arg(args, "trial_id")?;
                let room_id = str_arg(args, "room_id")?;
                let preview = app
                    .light_scene_previews
                    .get(&preview_id)
                    .filter(|p| {
                        p.target_node_id == room_id
                            && p.expires_at_epoch_ms > crate::state::current_epoch_ms()
                    })
                    .ok_or_else(stale)?;
                Self::KeepScene {
                    room_id,
                    preview_id,
                    scene_id: preview.scene_id.clone(),
                }
            }
            "home_scene" => {
                let id = str_arg(args, "scene_id")?;
                if !app.scenes.contains_key(&id) || crate::scenes::is_native_scene_id(&id) {
                    return Err(invalid("Select a saved whole-home scene."));
                }
                Self::HomeScene {
                    id,
                    target_mode: parse(&args["target_mode"])?,
                }
            }
            "scene" => {
                let id = str_arg(args, "scene_id")?;
                if crate::scenes::is_native_scene_id(&id) {
                    return Err(invalid("Edit native scenes in their owning integration."));
                }
                let mut definition: Option<SceneDefinition> = parse(&args["definition"])?;
                if let Some(scene) = &mut definition {
                    scene.normalize();
                    let layer = scene
                        .light
                        .as_ref()
                        .ok_or_else(|| invalid("Choose a scene with a light layer."))?;
                    if scene.id != id {
                        return Err(invalid("Choose a matching scene ID."));
                    }
                    let mut outputs = layer
                        .default_output
                        .iter()
                        .chain(&layer.palette)
                        .chain(layer.entries.iter().map(|entry| &entry.output))
                        .peekable();
                    if outputs.peek().is_none() {
                        return Err(invalid("A light scene needs at least one output."));
                    }
                    for output in outputs {
                        output
                            .to_command(layer.default_transition_ms)
                            .map_err(invalid)?;
                    }
                } else if !app.scenes.contains_key(&id) {
                    return Err(invalid("This scene does not exist."));
                }
                Self::Scene { id, definition }
            }
            "light_schedule" => {
                let id = str_arg(args, "schedule_id")?;
                let definition: Option<LightScheduleConfig> = parse(&args["definition"])?;
                if let Some(schedule) = &definition {
                    if schedule.id != id {
                        return Err(invalid("Schedule ID mismatch."));
                    }
                    commands::validate_light_schedule_definitions(std::slice::from_ref(schedule))
                        .map_err(invalid_model)?;
                } else if !app.light_schedules.contains_key(&id) {
                    return Err(invalid("This schedule does not exist."));
                }
                Self::Schedule { id, definition }
            }
            "input_binding" => {
                let id = str_arg(args, "binding_id")?;
                let binding: Option<InputBinding> = parse(&args["binding"])?;
                if let Some(binding) = &binding {
                    if binding.id != id {
                        return Err(invalid("Binding ID mismatch."));
                    }
                    commands::validate_input_binding_source(app, &binding.source_node_id)
                        .map_err(invalid_model)?;
                    commands::validate_automation_action(&binding.action).map_err(invalid_model)?;
                } else if app.topology.input_binding(&id).is_none() {
                    return Err(invalid("This binding does not exist."));
                }
                Self::Binding { id, binding }
            }
            "mode_transitions" => {
                let list: Vec<ModeTransitionConfig> = parse(&args["transitions"])?;
                if list.len() > 128 {
                    return Err(invalid("Too many transitions."));
                }
                Self::Transitions(rhythm_core::normalize_mode_transition_configs(list))
            }
            "mode_settings" => {
                let list: Vec<ModeConfig> = parse(&args["modes"])?;
                if list.len() > 2 {
                    return Err(invalid("Select at most two modes."));
                }
                commands::validate_mode_configs(&list).map_err(invalid_model)?;
                Self::Modes(list)
            }
            "schedule_assignment" => {
                let node = str_arg(args, "node_id")?;
                let authority = str_arg(args, "authority")?;
                let schedule = match authority.as_str() {
                    "named" => {
                        let id = str_arg(args, "schedule_id")?;
                        if !app.light_schedules.contains_key(&id) {
                            return Err(invalid("Select an existing schedule."));
                        }
                        Some(id)
                    }
                    "legacy" | "unscheduled" if args.get("schedule_id").is_none() => None,
                    _ => return Err(invalid("Choose named, legacy or unscheduled authority.")),
                };
                Self::Assignment {
                    node,
                    authority,
                    schedule,
                }
            }
            "schedule_override" => Self::ScheduleOverride {
                node: str_arg(args, "node_id")?,
                schedule: str_arg(args, "schedule_id")?,
                value: parse(&args["override"])?,
            },
            "node_preferences" => {
                let profile = args.get("profile_settings").cloned();
                if let Some(p) = profile.as_ref() {
                    if !p.is_object() {
                        return Err(invalid("Expected a profile settings object."));
                    }
                    crate::handlers::parse_profile_settings_patch(Some(p), "profile_settings")
                        .map_err(invalid)?;
                }
                let opt_bool = |key: &str| args.get(key).map(parse::<bool>).transpose();
                let rhythm = opt_bool("rhythm_enabled")?;
                let disabled = opt_bool("disabled")?;
                let standby = opt_bool("standby_enabled")?;
                let mode = args.get("state").map(parse::<RoomModeState>).transpose()?;
                if rhythm.is_none()
                    && disabled.is_none()
                    && standby.is_none()
                    && mode.is_none()
                    && profile.is_none()
                {
                    return Err(invalid("Choose a preference to change."));
                }
                Self::Preferences {
                    node: str_arg(args, "node_id")?,
                    rhythm,
                    disabled,
                    standby,
                    mode,
                    profile,
                }
            }
            "light_breaker" => Self::Breaker(parse(&args["enabled"])?),
            "light_runtime" => {
                let runtime = str_arg(args, "runtime_id")?;
                Self::Runtime(
                    app.light_runtime_registry
                        .parse_runtime_kind(&runtime)
                        .map_err(invalid_model)?
                        .as_str()
                        .into(),
                )
            }
            "update_settings" => {
                if app.platform_context == "ha_addon" {
                    return Err(invalid("Review add-on updates in Home Assistant."));
                }
                Self::Updates {
                    auto: parse(&args["auto_update"])?,
                    channel: parse(&args["update_channel"])?,
                }
            }
            _ => return Err(invalid("Unknown lighting automation.")),
        };
        Ok(edit)
    }

    fn node(&self) -> Option<&str> {
        match self {
            Self::Assignment { node, .. }
            | Self::Preferences { node, .. }
            | Self::ScheduleOverride { node, .. } => Some(node),
            _ => None,
        }
    }
    fn snapshot(&self, app: &AppState) -> Result<Value> {
        if let Some(id) = self.node() {
            let runtime = app
                .hub_runtime()
                .ok_or_else(|| invalid("Lighting runtime is offline."))?;
            if !app.topology.has_public_node(id) || !app.rhythm_automation_allowed_for_node(id) {
                return Err(invalid("Select a Rhythm-managed light or room."));
            }
            let n = runtime
                .engine_node_snapshot(id)
                .filter(|n| n.kind.is_light_addressable())
                .ok_or_else(|| invalid("Lighting target is unavailable."))?;
            let e = runtime
                .engine_effective_node_snapshot(id)
                .ok_or_else(stale)?;
            return Ok(
                json!({"node_id":id,"parent_id":n.parent_id,"local_profile_settings":n.profile_settings,"effective_profile_settings":e.profile_settings,"disabled":n.disabled,"rhythm_enabled":n.rhythm_enabled,"standby_enabled":n.standby_enabled,"hard_off":n.hard_off,"soft_off":n.soft_off,"mood_active":n.mood_active}),
            );
        }
        Ok(match self {
            Self::KeepScene {
                room_id, scene_id, ..
            } => {
                json!({"scene":app.scenes.get(scene_id),"room_binding":app.hub_runtime().and_then(|rt|rt.engine_node_snapshot(room_id)).map(|s|s.profile_settings.mood_scene_id)})
            }
            Self::Scene { id, .. } | Self::HomeScene { id, .. } => json!(app.scenes.get(id)),
            Self::Schedule { id, .. } => json!(app.light_schedules.get(id)),
            Self::Binding { id, .. } => json!(app.topology.input_binding(id)),
            Self::Transitions(_) => json!(app.mode_transition_configs()),
            Self::Modes(_) => json!(app.mode_configs()),
            Self::Breaker(_) => json!(app.light_breaker_enabled),
            Self::Runtime(_) => json!(app.light_runtime_kind.as_str()),
            Self::Updates { .. } => {
                json!({"auto_update":app.auto_update,"update_channel":app.update_channel})
            }
            _ => unreachable!("node edits returned above"),
        })
    }
    fn affected_rooms(&self, app: &AppState) -> Vec<String> {
        if let Self::KeepScene { room_id, .. } = self {
            return vec![room_id.clone()];
        }
        if let Some(id) = self.node() {
            return vec![app
                .topology
                .device_parent_room_id(id)
                .unwrap_or(id)
                .to_owned()];
        }
        // A binding can target hub-wide modes. A scene or schedule definition
        // can be selected on these rooms even if only some use it right now.
        room_ids(app)
    }
    fn evidence(&self, app: &AppState) -> Result<Value> {
        let current = self.snapshot(app)?;
        if let Self::KeepScene {
            room_id,
            preview_id,
            ..
        } = self
        {
            let preview = app
                .light_scene_previews
                .get(preview_id)
                .filter(|p| {
                    p.target_node_id == *room_id
                        && p.expires_at_epoch_ms > crate::state::current_epoch_ms()
                })
                .ok_or_else(stale)?;
            if !app.rhythm_automation_allowed_for_node(room_id) {
                return Err(stale());
            }
            return Ok(
                json!({"current":current,"preview":preview,"affected_node_ids":preview.affected_node_ids}),
            );
        }
        let mut bindings = BTreeMap::new();
        if let Some(rt) = app.hub_runtime() {
            for node in rt.engine_all_node_snapshots() {
                match self {
                    Self::HomeScene { .. }
                        if node.kind.is_light_addressable()
                            && !crate::topology::is_internal_light_node_id(&node.id) =>
                    {
                        bindings.insert(node.id.clone(),json!({"parent_id":node.parent_id,"disabled":node.disabled,"managed":app.rhythm_automation_allowed_for_node(&node.id) && app.canonical_registry.get(&node.id).is_none_or(|light|app.ha_selection_allows_light(light)),"previous_scene":node.profile_settings.mood_scene_id,"mood_active":node.mood_active}));
                    }
                    Self::Scene { id, .. }
                        if node.profile_settings.mood_scene_id.as_ref() == Some(id) =>
                    {
                        bindings.insert(
                            node.id,
                            json!({"mood_scene_id":id,"mood_active":node.mood_active}),
                        );
                    }
                    Self::Schedule { id, .. }
                        if node
                            .profile_settings
                            .light_schedule
                            .as_ref()
                            .and_then(|a| a.schedule_id())
                            == Some(id)
                            || node
                                .profile_settings
                                .light_schedule_overrides
                                .contains_key(id) =>
                    {
                        bindings.insert(node.id,json!({"assignment":node.profile_settings.light_schedule,"customization":node.profile_settings.light_schedule_overrides.get(id)}));
                    }
                    _ => {}
                }
            }
        }
        Ok(json!({"current":current,"room_ids":self.affected_rooms(app),"bound_nodes":bindings}))
    }
}

impl Edit {
    fn apply(
        &self,
        state: &SharedState,
        check: ReviewCheck<'_>,
        token: &str,
    ) -> anyhow::Result<Option<HomeSceneApplyResponse>> {
        match self {
            Self::KeepScene { preview_id, .. } => {
                commands::do_scene_preview_commit_reviewed(state, preview_id, check)?
            }
            Self::HomeScene { id, target_mode } => {
                let request = crate::scenes::HomeSceneApplyRequest {
                    target_mode: *target_mode,
                    transition_ms: None,
                };
                let result = commands::do_home_scene_apply_reviewed(
                    state,
                    id,
                    request,
                    commands::default_http_batch_dispatch_spacing(),
                    Some(check),
                )?;
                return Ok(Some(result));
            }
            Self::Scene { id, definition } => match definition {
                Some(scene) => commands::do_scene_upsert_reviewed(state, scene.clone(), check)?,
                None => commands::do_scene_delete_reviewed(state, id, check)?,
            },
            Self::Binding { id, binding } => match binding {
                Some(binding) => {
                    commands::do_input_binding_set_reviewed(state, binding.clone(), check)?
                }
                None => commands::do_input_binding_delete_reviewed(state, id, check)?,
            },
            Self::Schedule { id, definition } => {
                let before = {
                    let app = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
                    check(&app)?;
                    app.light_schedule_configs()
                };
                let mut updated = before.clone();
                updated.retain(|s| s.id != *id);
                if let Some(schedule) = definition {
                    updated.push(schedule.clone());
                }
                commands::do_light_schedules_set_guarded_with_correlation(
                    state,
                    updated,
                    before,
                    Some(token.into()),
                )?
            }
            Self::Modes(modes) => commands::do_settings_set_internal_reviewed(
                state,
                None,
                None,
                Some(modes.clone()),
                None,
                None,
                false,
                None,
                None,
                check,
            )?,
            Self::Transitions(transitions) => commands::do_settings_set_internal_reviewed(
                state,
                None,
                None,
                None,
                Some(transitions.clone()),
                None,
                false,
                None,
                None,
                check,
            )?,
            Self::Updates { auto, channel } => commands::do_settings_set_internal_reviewed(
                state,
                None,
                None,
                None,
                None,
                None,
                false,
                Some(*auto),
                Some(*channel),
                check,
            )?,
            Self::Breaker(enabled) => {
                commands::do_light_breaker_set_reviewed(state, *enabled, check)?
            }
            Self::Runtime(id) => commands::do_light_runtime_settings_set_reviewed(
                state,
                crate::light_runtime::LightRuntimeKind::new(id),
                check,
            )?,
            Self::Assignment {
                node,
                authority,
                schedule,
            } => {
                let mutex = state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("lock"))?
                    .light_schedule_write_lock
                    .clone();
                let _writer = mutex.lock().map_err(|_| anyhow::anyhow!("schedule lock"))?;
                let selection = match (authority.as_str(), schedule) {
                    ("legacy", _) => commands::LightScheduleAuthority::Legacy,
                    ("named", Some(id)) => commands::LightScheduleAuthority::Named(id),
                    _ => commands::LightScheduleAuthority::Unscheduled,
                };
                commands::do_light_schedule_authority_set_reviewed(
                    state,
                    node,
                    selection,
                    Some(token.into()),
                    true,
                    chrono::Utc::now().naive_utc(),
                    check,
                )?
            }
            Self::ScheduleOverride {
                node,
                schedule,
                value,
            } => {
                let expected = {
                    let app = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
                    check(&app)?;
                    app.hub_runtime()
                        .and_then(|r| r.engine_effective_node_snapshot(node))
                        .ok_or_else(|| anyhow::anyhow!("missing target"))?
                        .profile_settings
                        .light_schedule_overrides
                };
                commands::do_light_schedule_override_set(
                    state,
                    node,
                    schedule,
                    value.clone(),
                    Some(expected),
                    Some(token.into()),
                    true,
                )?
            }
            Self::Preferences {
                node,
                rhythm,
                disabled,
                standby,
                mode,
                profile,
            } => {
                let patch = crate::handlers::parse_profile_settings_patch(
                    profile.as_ref(),
                    "profile_settings",
                )
                .map_err(anyhow::Error::msg)?;
                commands::do_node_preferences_set_inner_reviewed(
                    state,
                    node,
                    *rhythm,
                    *disabled,
                    *standby,
                    *mode,
                    patch.as_ref(),
                    true,
                    true,
                    check,
                )?
            }
        };
        Ok(None)
    }
}

pub(super) fn call(state: &SharedState, id: &str, args: Value) -> Result<Value> {
    let mut app = state
        .lock()
        .map_err(|_| Error::internal("state_unavailable", "Hub state unavailable."))?;
    if id == "read_automation_settings" {
        return Ok(settings(&app));
    }
    if let Some(kind) = id
        .strip_prefix("plan_")
        .and_then(|s| s.strip_suffix("_edit"))
    {
        let edit = Edit::parse(&app, kind, &args)?;
        let before = edit.evidence(&app)?;
        let affected = edit.affected_rooms(&app);
        let token = uuid::Uuid::new_v4().to_string();
        let operation = format!("apply_{kind}_edit");
        let plan = AutomationPlan {
            token: token.clone(),
            operation: operation.clone(),
            server: app.server_instance_id.clone(),
            valid_until: Instant::now() + Duration::from_secs(300),
            before: before.clone(),
            edit,
        };
        let preview = json!({"plan_id":token,"operation":operation,"server_instance_id":app.server_instance_id,
            "before":before,"proposal":plan.edit,"affected_rooms":affected,"requires_confirmation":true});
        app.assistant_automation_plans
            .retain(|p| p.valid_until > Instant::now());
        if app.assistant_automation_plans.len() >= 32 {
            app.assistant_automation_plans.pop_front();
        }
        app.assistant_automation_plans.push_back(plan);
        return Ok(preview);
    }
    if args["confirmed"] != true {
        return Err(invalid("Explicit confirmation is required."));
    }
    let token = str_arg(&args, "plan_id")?;
    let idx = app
        .assistant_automation_plans
        .iter()
        .position(|p| p.token == token && p.operation == id)
        .ok_or_else(stale)?;
    let plan = app
        .assistant_automation_plans
        .remove(idx)
        .ok_or_else(stale)?;
    if plan.valid_until <= Instant::now() {
        return Err(stale());
    }
    drop(app);
    let check = |app: &AppState| {
        if app.server_instance_id != plan.server
            || plan.edit.evidence(app).ok().as_ref() != Some(&plan.before)
        {
            anyhow::bail!("automation precondition failed");
        }
        Ok(())
    };
    let outcome = plan.edit.apply(state, &check, &token).map_err(|e| {
        if e.to_string().contains("precondition failed") {
            return stale();
        }
        Error {
            status: 409,
            code: "automation_edit_unverified",
            message:
                "Could not verify this edit. Read the current configuration and review it again."
                    .into(),
            retryable: false,
            mutation_may_have_applied: true,
        }
    })?;
    let after = plan.edit.snapshot(&*state.lock().map_err(|_| {
        Error::internal(
            "state_unavailable",
            "Read current state before repeating this operation.",
        )
    })?)?;
    let mut record =
        crate::activity::LightActivityRecord::app(plan.edit.node().unwrap_or("global"), id);
    record.correlation_id = Some(token.clone());
    crate::activity::record_light_activity(state, record);
    let mut status = "applied";
    let dispatch = outcome.map(|result| {
        let failed = result.targets.iter().filter(|t| t.error.is_some()).count();
        let unresolved = result.targets.iter().any(|t| !t.unresolved_node_ids.is_empty());
        if failed > 0 || unresolved {
            status = "partially_applied";
        } else if result.queued {
            status = "queued";
        }
        let targets = result.targets.iter().map(|t| json!({
            "target_id":t.target_id,
            "applied":t.error.is_none(),
            "affected_node_ids":t.affected_node_ids,
            "unresolved_node_ids":t.unresolved_node_ids
        })).collect::<Vec<_>>();
        json!({"queued":result.queued,"targets":targets,"applied_target_count":result.applied_target_count,
            "failed_target_count":failed,"skipped_target_count":result.skipped_target_count})
    });
    Ok(
        json!({"status":status,"plan_id":token,"operation":id,"server_instance_id":plan.server,"after":after,"dispatch":dispatch,"physical_verification":"not_observed"}),
    )
}
