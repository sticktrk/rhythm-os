//! Portable lighting behavior. Installation recovery and secrets remain owner actions.
use super::*;
use crate::bundle::{LightingSettingsImportPayload, ProfileBundleImportPayload};
use crate::state::AppState;
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, LightAssistantError>;

#[derive(Debug)]
pub struct TransferPlan {
    pub(super) id: String,
    installation: String,
    revision: String,
    action: &'static str,
    payload: Value,
    created: Instant,
}

fn unavailable() -> LightAssistantError {
    LightAssistantError::internal(
        "state_unavailable",
        "Could not read portable lighting settings",
    )
}
fn invalid() -> LightAssistantError {
    LightAssistantError::bad_request(
        "invalid_transfer",
        "Select a supported portable lighting bundle and review its node mappings",
    )
}
fn stale() -> LightAssistantError {
    LightAssistantError::conflict(
        "transfer_changed",
        "The installation or its lighting settings changed. Review the transfer again.",
    )
}

fn revision(s: &AppState) -> String {
    let nodes: std::collections::BTreeMap<_, _> = s
        .hub_runtime()
        .into_iter()
        .flat_map(|r| r.engine_all_node_snapshots())
        .map(|n| {
            (
                n.id,
                json!([
                    n.name,
                    n.parent_id,
                    n.kind,
                    n.profile_settings,
                    n.rhythm_enabled,
                    n.disabled,
                    n.standby_enabled
                ]),
            )
        })
        .collect();
    canonical_json_sha256(&json!([
        s.light_profile_configs,
        s.scenes,
        s.mode_configs(),
        s.mode_transition_configs(),
        s.light_schedule_configs(),
        s.power_save,
        s.light_breaker_enabled,
        s.managed_ha_lights,
        commands::build_topology_node_dtos(s),
        nodes
    ]))
}

pub(super) fn operations() -> Vec<LightAssistantOperationDto> {
    let settings = json!({"type":"object","description":"A portable lighting_settings or profile_bundle export. Use the owner restore flow for installation backups."});
    let object = || json!({"type":"object"});
    let mut ops = vec![
        operation("read_activity_upload","Read whether the selected hub is configured to upload lighting activity.",OperationTier::Read,object_schema(&[],json!({})),object()),
        operation("plan_activity_upload","Review enabling or pausing lighting activity uploads to the already configured home. The owner must provision a new destination in the app.",OperationTier::Read,object_schema(&["enabled"],json!({"enabled":{"type":"boolean"}})),object()),
        operation("export_lighting_settings","Export portable lighting preferences, scenes, profiles, schedules and source room labels for mapping to a destination. Keep this export with the resident.", OperationTier::Read,object_schema(&[],json!({})),object()),
        operation("read_factory_profiles","Read the factory lighting profiles, scenes, schedules and transitions before reviewing reset.",OperationTier::Read,object_schema(&[],json!({})),object()),
        operation("plan_lighting_transfer","Preview merging a portable bundle. Review the destination mapping, resulting definitions, skipped nodes and whether adaptation will pause. Only explicitly mapped source nodes transfer their preferences.",OperationTier::Read,
            object_schema(&["settings","node_mappings"],json!({"settings":settings,"node_mappings":{"type":"object","additionalProperties":{"type":"string"},"description":"Map source node IDs to existing destination IDs. Use an empty object to transfer definitions alone."}})),object()),
        operation("plan_profile_bundle_reset","Review replacement of the hub's profile bundle with factory profiles, scenes, schedules and transitions. This changes shared behavior across rooms.",OperationTier::Read,object_schema(&[],json!({})),object()),
        operation("plan_profile_edit","Review changes to one existing shared lighting profile. Supply its full definition. Use read_profile_curve to audition the calculation and plan_room_tuning for a room-specific preference.",OperationTier::Read,object_schema(&["profile_id","profile"],json!({"profile_id":{"type":"string"},"profile":{"type":"object"}})),object()),
        operation("plan_profile_reset","Review restoring one factory profile on every room using it.",OperationTier::Read,object_schema(&["profile_id"],json!({"profile_id":{"type":"string"}})),object()),
    ];
    for (id, desc) in [
        (
            "apply_activity_upload",
            "Apply the reviewed activity upload preference.",
        ),
        (
            "apply_lighting_transfer",
            "Apply the reviewed portable lighting transfer to this installation.",
        ),
        (
            "apply_profile_bundle_reset",
            "Replace the reviewed profile bundle with factory lighting behavior.",
        ),
        (
            "apply_profile_edit",
            "Save the reviewed definition of one shared lighting profile.",
        ),
        (
            "apply_profile_reset",
            "Restore the reviewed single profile to its factory settings.",
        ),
    ] {
        ops.push(operation(
            id,
            desc,
            OperationTier::PersistentChange,
            object_schema(
                &["plan_id", "confirmed"],
                json!({"plan_id":{"type":"string"},"confirmed":{"const":true,"type":"boolean"}}),
            ),
            object(),
        ));
    }
    ops
}

pub(super) fn call(state: &SharedState, id: &str, args: Value) -> Result<Value> {
    if id.starts_with("apply_") {
        return apply(state, id, &args);
    }
    if id == "read_activity_upload" || id == "plan_activity_upload" {
        let (before, revision) = crate::activity_cloud::assistant_upload_preference(state, None)
            .map_err(|_| unavailable())?;
        if id == "read_activity_upload" {
            return Ok(before);
        }
        let enabled = args["enabled"].as_bool().ok_or_else(invalid)?;
        let mut s = state.lock().map_err(|_| unavailable())?;
        let hub = s.server_instance_id.clone();
        if before["configured"] != true || (enabled && before["needs_reprovision"] == true) {
            return Ok(
                json!({"status":"handoff","actor":"owner","surface":"rhythm_app","instructions":"Connect activity sharing for this home in the Rhythm app.","server_instance_id":hub}),
            );
        }
        let id = uuid::Uuid::new_v4().to_string();
        if s.assistant_transfers.len() >= 8 {
            s.assistant_transfers.pop_front();
        }
        s.assistant_transfers.push_back(TransferPlan {
            id: id.clone(),
            installation: hub.clone(),
            revision,
            action: "apply_activity_upload",
            payload: json!(enabled),
            created: Instant::now(),
        });
        return Ok(
            json!({"plan_id":id,"requires_confirmation":true,"server_instance_id":hub,"operation":"apply_activity_upload","before":before,"after":{"enabled":enabled}}),
        );
    }
    let settings = || {
        commands::build_lighting_settings_bundle(state)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .ok_or_else(unavailable)
    };
    if id == "export_lighting_settings" {
        return settings();
    }
    if id == "read_factory_profiles" {
        return Ok(json!(commands::build_factory_default_profile_bundle_dto()));
    }
    let (installation, revision_before) = {
        let s = state.lock().map_err(|_| unavailable())?;
        (s.server_instance_id.clone(), revision(&s))
    };
    let before = settings()?;
    let (action, payload, review) = match id {
        "plan_lighting_transfer" => {
            let kind = args["settings"]["kind"].as_str();
            if !matches!(
                kind,
                Some(
                    "lighting_settings"
                        | "profile_bundle"
                        | "share_bundle"
                        | "configuration_bundle"
                )
            ) || args.to_string().len() > 256 * 1024
            {
                return Err(invalid());
            }
            let normalized = commands::normalize_lighting_settings_payload(&args["settings"])
                .map_err(|_| invalid())?;
            let payload = json!({"settings":normalized,"node_mappings":args["node_mappings"]});
            let transfer: LightingSettingsImportPayload =
                serde_json::from_value(payload.clone()).map_err(|_| invalid())?;
            {
                let app = state.lock().map_err(|_| unavailable())?;
                if transfer.node_mappings.values().any(|id| {
                    !app.topology.has_public_node(id) || !app.assistant_lighting_target_allowed(id)
                }) {
                    return Err(LightAssistantError::bad_request(
                        "unmanaged_destination",
                        "Map settings only to public lights or rooms delegated to Rhythm.",
                    ));
                }
            }
            let preview =
                commands::review_lighting_settings_import(state, transfer, true, &|_| Ok(()))
                    .map_err(|_| invalid())?;
            (
                "apply_lighting_transfer",
                payload,
                serde_json::from_str(&preview).map_err(|_| unavailable())?,
            )
        }
        "plan_profile_bundle_reset" => {
            let bundle: Value = serde_json::from_str(
                &commands::build_factory_default_profile_bundle().map_err(|_| unavailable())?,
            )
            .map_err(|_| unavailable())?;
            let app = state.lock().map_err(|_| unavailable())?;
            let defaults = commands::build_factory_default_profile_bundle_dto();
            let removed_scene_ids = app
                .scenes
                .keys()
                .filter(|id| !defaults.profile.scenes.iter().any(|scene| &scene.id == *id))
                .cloned()
                .collect::<std::collections::BTreeSet<_>>();
            let bound_nodes: Vec<_> = app
                .hub_runtime()
                .into_iter()
                .flat_map(|runtime| runtime.engine_all_node_snapshots())
                .filter(|node| {
                    node.profile_settings
                        .mood_scene_id
                        .as_ref()
                        .is_some_and(|id| removed_scene_ids.contains(id))
                })
                .map(|node| node.id)
                .collect();
            if !bound_nodes.is_empty() {
                return Err(LightAssistantError::bad_request(
                    "scene_bindings_in_use",
                    format!("Reset would remove saved scenes used by these nodes: {}. Change their Mood bindings before reviewing a reset.", bound_nodes.join(", ")),
                ));
            }
            (
                "apply_profile_bundle_reset",
                Value::Null,
                json!({"before":before["profile"], "after":bundle["profile"],"removed_scene_ids":removed_scene_ids}),
            )
        }
        "plan_profile_edit" | "plan_profile_reset" => {
            let profile = args["profile_id"].as_str().ok_or_else(invalid)?;
            let current = commands::build_config(state, Some(profile)).map_err(|_| invalid())?;
            let current: Value = serde_json::from_str(&current).map_err(|_| unavailable())?;
            let (action, mut updated) = if id == "plan_profile_reset" {
                (
                    "apply_profile_reset",
                    crate::factory_default_config::factory_default_light_profile_config(profile)
                        .ok_or_else(invalid)?,
                )
            } else {
                let config: rhythm_core::LightProfileConfig =
                    serde_json::from_value(args["profile"].clone()).map_err(|_| invalid())?;
                if config.id != profile {
                    return Err(invalid());
                }
                ("apply_profile_edit", config)
            };
            rhythm_core::normalize_builtin_state_profile_config(&mut updated);
            updated.normalize_float_precision();
            let updated =
                commands::light_profile_config_resource(&updated).map_err(|_| unavailable())?;
            (
                action,
                updated.clone(),
                json!({"before":current,"after":updated}),
            )
        }
        _ => return Err(invalid()),
    };
    let mut s = state.lock().map_err(|_| unavailable())?;
    if installation != s.server_instance_id || revision_before != revision(&s) {
        return Err(stale());
    }
    let rooms: Vec<_> = s
        .topology
        .rooms()
        .map(|r| json!({"id":r.id,"name":r.name}))
        .collect();
    let token = uuid::Uuid::new_v4().to_string();
    s.assistant_transfers
        .retain(|p| p.created.elapsed() < Duration::from_secs(300));
    if s.assistant_transfers.len() >= 8 {
        s.assistant_transfers.pop_front();
    }
    s.assistant_transfers.push_back(TransferPlan {
        id: token.clone(),
        installation: installation.clone(),
        revision: revision_before,
        action,
        payload,
        created: Instant::now(),
    });
    Ok(
        json!({"plan_id":token,"server_instance_id":installation,"operation":action,"requires_confirmation":true,"potentially_affected_rooms":rooms,"review":review,
        "pauses_adaptation":s.platform_context=="ha_addon" && matches!(action,"apply_lighting_transfer"|"apply_profile_bundle_reset")}),
    )
}

fn apply(state: &SharedState, id: &str, args: &Value) -> Result<Value> {
    let plan = {
        let mut s = state.lock().map_err(|_| unavailable())?;
        let pos = s
            .assistant_transfers
            .iter()
            .position(|p| args["plan_id"].as_str() == Some(p.id.as_str()) && p.action == id)
            .ok_or_else(stale)?;
        s.assistant_transfers.remove(pos).ok_or_else(stale)?
    };
    if id == "apply_activity_upload" {
        if plan.created.elapsed() >= Duration::from_secs(300)
            || state.lock().map_err(|_| unavailable())?.server_instance_id != plan.installation
        {
            return Err(stale());
        }
        let (readback, _) = crate::activity_cloud::assistant_upload_preference(
            state,
            Some((&plan.revision, plan.payload.as_bool().ok_or_else(invalid)?)),
        )
        .map_err(|e| {
            if e.to_string() == "activity_changed" {
                stale()
            } else {
                unavailable()
            }
        })?;
        return Ok(
            json!({"status":"applied","operation":id,"plan_id":plan.id,"server_instance_id":plan.installation,"after":readback}),
        );
    }
    let check = |s: &AppState| -> anyhow::Result<()> {
        anyhow::ensure!(
            plan.created.elapsed() < Duration::from_secs(300)
                && s.server_instance_id == plan.installation
                && revision(s) == plan.revision
                && plan.payload["node_mappings"]
                    .as_object()
                    .is_none_or(|mappings| mappings.values().all(|id| id
                        .as_str()
                        .is_some_and(|id| s.topology.has_public_node(id)
                            && s.assistant_lighting_target_allowed(id)))),
            "transfer_changed"
        );
        Ok(())
    };
    let result = match id {
        "apply_lighting_transfer" => commands::review_lighting_settings_import(
            state,
            serde_json::from_value(plan.payload.clone()).map_err(|_| invalid())?,
            false,
            &check,
        ),
        "apply_profile_bundle_reset" => commands::do_profile_bundle_import_reviewed(
            state,
            ProfileBundleImportPayload::Bundle(commands::build_factory_default_profile_bundle_dto()),
            &check,
        ),
        "apply_profile_edit" | "apply_profile_reset" => {
            let config = serde_json::from_value(plan.payload.clone()).map_err(|_| invalid())?;
            commands::do_config_set_reviewed(state, config, false, &check)
                .and_then(|_| commands::build_config(state, plan.payload["id"].as_str()))
        }
        _ => return Err(invalid()),
    };
    let raw = result.map_err(|e| {
        if e.to_string() == "transfer_changed" {
            stale()
        } else {
            LightAssistantError {
                status: 409,
                code: "transfer_not_verified",
                message: "Reread portable lighting settings and review a new plan before retrying."
                    .into(),
                retryable: false,
                mutation_may_have_applied: true,
                trial_id: None,
            }
        }
    })?;
    let output: Value = serde_json::from_str(&raw).map_err(|_| unavailable())?;
    Ok(
        json!({"status":"applied","plan_id":plan.id,"server_instance_id":plan.installation,"operation":id,"canonical_readback":output}),
    )
}
