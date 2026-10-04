//! Reviewed transfer of lighting behavior without transferring device ownership.
use super::*;
use crate::bundle::{LightingSettingsBundle, LightingSettingsImportPayload, LightingSettingsNode};

const KIND: &str = "lighting_settings";
const SCHEMA: u32 = 1;
const WARNING_CODES: &[&str] = &[
    "native_scenes_excluded",
    "scene_extensions_excluded",
    "native_scene_bindings_excluded",
    "unmapped_scene_entries_excluded",
    "unmapped_mode_defaults_excluded",
    "newer_target_enablement_preserved",
];

fn clear_schedule_mode(settings: &mut RoomProfileSettings, mode: RhythmMode) {
    settings.light_schedule_modes.clear();
    if let Some(assignment) = &mut settings.light_schedule {
        match assignment {
            rhythm_core::LightScheduleAssignment::Named { active_mode, .. }
            | rhythm_core::LightScheduleAssignment::Unscheduled { active_mode } => {
                *active_mode = mode;
            }
        }
    }
}

fn portable_settings(mut settings: LightingSettingsBundle) -> Result<LightingSettingsBundle> {
    anyhow::ensure!(
        settings.kind == KIND && settings.schema_version == SCHEMA,
        "Unsupported lighting settings bundle"
    );
    anyhow::ensure!(
        settings.nodes.len() <= 4096,
        "Too many lighting settings nodes"
    );
    let mut warnings: BTreeSet<String> = settings
        .warnings
        .into_iter()
        .filter(|warning| WARNING_CODES.contains(&warning.as_str()))
        .collect();
    let mut excluded_scenes = HashSet::new();
    settings.profile.scenes.retain_mut(|scene| {
        if is_native_scene_id(&scene.id) || matches!(scene.source, SceneSource::Imported { .. }) {
            excluded_scenes.insert(scene.id.clone());
            warnings.insert("native_scenes_excluded".into());
            return false;
        }
        if !scene.extensions.is_empty() {
            scene.extensions.clear();
            warnings.insert("scene_extensions_excluded".into());
        }
        true
    });
    let mut node_ids = HashSet::new();
    for node in &mut settings.nodes {
        anyhow::ensure!(
            !node.id.trim().is_empty() && node_ids.insert(node.id.clone()),
            "Lighting settings nodes require unique nonempty IDs"
        );
        anyhow::ensure!(
            node.kind.is_light_addressable(),
            "Unsupported lighting settings node kind"
        );
        let profile = &mut node.room_profile;
        if profile
            .mood_scene_id
            .as_ref()
            .is_some_and(|id| is_native_scene_id(id) || excluded_scenes.contains(id))
        {
            profile.mood_scene_id = None;
            profile.mood_scene_palette_offset = None;
            profile.mood_scene_palette_span = None;
            profile.mood_scene_palette_seed = None;
            warnings.insert("native_scene_bindings_excluded".into());
        }
        // The shape uses the existing typed assignment contract; its required
        // mode is a placeholder. The target supplies its own mode on import.
        clear_schedule_mode(profile, RhythmMode::default());
    }
    if let Some(schedules) = &mut settings.profile.light_schedules {
        for schedule in schedules {
            schedule.active_mode = RhythmMode::default();
        }
    }
    settings.nodes.sort_by(|left, right| left.id.cmp(&right.id));
    settings.warnings = warnings.into_iter().collect();
    Ok(settings)
}

/// Normalize existing RPiZ backups and current portable settings through one
/// typed allowlist. This endpoint does not mutate any installation state.
pub fn normalize_lighting_settings_payload(body: &Value) -> Result<LightingSettingsBundle> {
    anyhow::ensure!(
        serde_json::to_vec(body)?.len() <= 32 * 1024 * 1024,
        "Lighting settings input exceeds supported size"
    );
    let legacy_backup = body.get("kind").is_none()
        && body.get("configuration").is_some_and(Value::is_object)
        && body.get("installation").is_some_and(Value::is_object)
        && body.get("runtime_state").is_some_and(Value::is_object)
        && body.get("created_at").is_some_and(Value::is_string);
    let kind = body
        .get("kind")
        .and_then(Value::as_str)
        .or_else(|| legacy_backup.then_some("backup_bundle"));
    let settings = match kind {
        Some(KIND) => serde_json::from_value(body.clone())?,
        Some("backup_bundle") => {
            let backup: BackupBundle = serde_json::from_value(body.clone())?;
            anyhow::ensure!(
                (1..=BACKUP_BUNDLE_SCHEMA_VERSION).contains(&backup.schema_version),
                "Unsupported backup schema version"
            );
            let mut nodes: BTreeMap<_, _> = backup
                .installation
                .rooms
                .iter()
                .filter(|room| room.kind.is_light_addressable())
                .map(|room| (room.id.clone(), LightingSettingsNode::from(room)))
                .collect();
            // Older/redacted backups may have only the portable room summary.
            for room in backup.configuration.rooms {
                nodes
                    .entry(room.id.clone())
                    .or_insert(LightingSettingsNode {
                        id: room.id,
                        name: room.name,
                        kind: LightNodeKind::Room,
                        parent_id: None,
                        rhythm_enabled: room.rhythm_enabled,
                        disabled: room.disabled,
                        standby_enabled: false,
                        room_profile: room.room_profile,
                    });
            }
            LightingSettingsBundle {
                schema_version: SCHEMA,
                kind: KIND.into(),
                profile: ProfileBundleData {
                    power_save: backup.configuration.power_save,
                    profiles: backup.configuration.profiles,
                    scenes: backup.configuration.scenes,
                    mode_transitions: backup.configuration.mode_transitions,
                    light_schedules: Some(backup.configuration.light_schedules),
                },
                mode_configs: Some(backup.configuration.mode_configs),
                nodes: nodes.into_values().collect(),
                warnings: Vec::new(),
            }
        }
        // A profile-only export is still useful; it simply has no node settings.
        Some("profile_bundle" | "share_bundle" | "configuration_bundle") => {
            let bundle: ProfileBundle = serde_json::from_value(body.clone())?;
            anyhow::ensure!(
                bundle.schema_version == PROFILE_BUNDLE_SCHEMA_VERSION,
                "Unsupported profile bundle schema version"
            );
            LightingSettingsBundle {
                schema_version: SCHEMA,
                kind: KIND.into(),
                profile: bundle.profile,
                mode_configs: None,
                nodes: Vec::new(),
                warnings: Vec::new(),
            }
        }
        _ => anyhow::bail!("Expected a lighting settings, backup, or profile bundle"),
    };
    portable_settings(settings)
}

pub fn build_lighting_settings_bundle(state: &SharedState) -> Result<String> {
    let rooms = source_room_manager_for_export(state);
    let app = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let bundle = portable_settings(LightingSettingsBundle {
        schema_version: SCHEMA,
        kind: KIND.into(),
        profile: profile_bundle_data_from_state(&app),
        mode_configs: Some(app.mode_configs()),
        nodes: rooms
            .iter()
            .filter(|room| room.kind.is_light_addressable())
            .map(LightingSettingsNode::from)
            .collect(),
        warnings: Vec::new(),
    })?;
    Ok(serde_json::to_string(&bundle)?)
}

/// Apply only reviewed preferences to existing nodes. All definition and
/// mapping checks run before pausing or writing, and no input/output command is
/// dispatched. The HA managed selection and canonical identity remain intact.
pub fn do_lighting_settings_import(
    state: &SharedState,
    payload: LightingSettingsImportPayload,
) -> Result<String> {
    let mut incoming = portable_settings(payload.settings)?;
    let mut warnings: BTreeSet<String> = incoming.warnings.into_iter().collect();
    let mappings = payload.node_mappings;
    let (scene_lock, topology_lock, schedule_lock) = {
        let app = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (
            app.scene_lifecycle_transaction_lock.clone(),
            app.external_topology_transaction_lock.clone(),
            app.light_schedule_write_lock.clone(),
        )
    };
    let mut targets = BTreeSet::new();
    let sources: BTreeMap<_, _> = incoming
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    for (source, target) in &mappings {
        anyhow::ensure!(
            sources.contains_key(source.as_str()),
            "Unknown source node '{}'",
            source
        );
        anyhow::ensure!(
            targets.insert(target.clone()),
            "Multiple source nodes map to '{}'",
            target
        );
    }
    let gates = {
        let mut app = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        targets
            .iter()
            .map(|id| node_command_gate(&mut app, id))
            .collect::<Vec<_>>()
    };
    // Schedule editors enter node preferences, which can enter scene dispatch
    // and topology. Follow that order. Nonblocking acquisition also avoids an
    // inversion with legacy whole-backup recovery's topology-first lease.
    let busy = || {
        anyhow::anyhow!(
            "Lighting settings are busy; finish the current lighting operation and review again"
        )
    };
    let _schedule_guard = schedule_lock.try_lock().map_err(|_| busy())?;
    let _node_guards = gates
        .iter()
        .map(|gate| gate.try_lock().map_err(|_| busy()))
        .collect::<Result<Vec<_>>>()?;
    let _scene_guard = scene_lock.try_lock().map_err(|_| busy())?;
    let _topology_guard = topology_lock.try_lock().map_err(|_| busy())?;
    let (runtime, mut profiles, mut scenes, mut schedules, mut modes, active_mode, is_ha) = {
        let app = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (
            app.hub_runtime(),
            app.light_profile_configs.clone(),
            app.scenes.clone(),
            app.light_schedules.clone(),
            app.mode_configs(),
            app.active_mode,
            app.platform_context == "ha_addon",
        )
    };
    anyhow::ensure!(
        runtime.is_some() || mappings.is_empty(),
        "Connect Home Assistant rooms and lights before mapping settings"
    );
    let mut snapshots = runtime
        .as_ref()
        .map(|runtime| runtime.engine_all_node_snapshots())
        .unwrap_or_default();
    let expected_enablement: HashMap<_, _> = snapshots
        .iter()
        .map(|node| (node.id.clone(), node.rhythm_enabled))
        .collect();
    let mut transition_ids = HashSet::new();
    for transition in
        rhythm_core::normalize_mode_transition_configs(incoming.profile.mode_transitions.clone())
    {
        anyhow::ensure!(
            transition_ids.insert(transition.id),
            "Duplicate mode transition id"
        );
    }
    let mut imported_profile_ids = HashSet::new();
    for profile in incoming.profile.profiles {
        anyhow::ensure!(
            !profile.id.trim().is_empty() && imported_profile_ids.insert(profile.id.clone()),
            "Profiles require unique nonempty IDs"
        );
        profiles.insert(profile.id.clone(), profile);
    }
    for scene in &mut incoming.profile.scenes {
        if let Some(light) = &mut scene.light {
            light.entries.retain_mut(|entry| {
                let LightSceneTargetRef::Node { node_id } = &mut entry.target;
                if let Some(target) = mappings.get(node_id) {
                    *node_id = target.clone();
                    true
                } else {
                    warnings.insert("unmapped_scene_entries_excluded".into());
                    false
                }
            });
        }
    }
    for (id, mut scene) in normalized_scene_map(incoming.profile.scenes)? {
        if let (Some(old_light), Some(new_light)) = (
            scenes.get(&id).and_then(|old| old.light.as_ref()),
            scene.light.as_mut(),
        ) {
            // A home can migrate in stages. Keep previously reviewed targets
            // when this transfer skips their source devices.
            new_light.entries.extend(
                old_light
                    .entries
                    .iter()
                    .filter(|entry| !targets.contains(entry.target.node_id()))
                    .cloned(),
            );
        }
        scenes.insert(id, scene);
    }
    if let Some(imported) = incoming.profile.light_schedules {
        validate_light_schedule_definitions(&imported)?;
        for mut schedule in imported {
            schedule.active_mode = schedules
                .get(&schedule.id)
                .map(|old| old.active_mode)
                .unwrap_or(active_mode);
            schedules.insert(schedule.id.clone(), schedule);
        }
    }
    let profile_ids = profiles.keys().cloned().collect();
    let scene_ids = scenes.keys().cloned().collect();
    for (source, target) in &mappings {
        let imported = sources[source.as_str()];
        let snapshot = snapshots
            .iter_mut()
            .find(|node| &node.id == target)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Target node '{}' no longer exists; review mappings again",
                    target
                )
            })?;
        anyhow::ensure!(
            snapshot.kind == imported.kind,
            "Cannot map different node kinds for '{}'",
            source
        );
        let mut profile = imported.room_profile.clone();
        clear_schedule_mode(
            &mut profile,
            snapshot
                .profile_settings
                .light_schedule
                .as_ref()
                .and_then(rhythm_core::LightScheduleAssignment::active_mode)
                .unwrap_or(active_mode),
        );
        validate_room_profile_settings(target, &profile, &profile_ids, Some(&scene_ids))?;
        for id in profile.profile_overrides.keys() {
            anyhow::ensure!(
                node_profile_override_id_allowed(snapshot.kind, id),
                "State profile '{}' cannot be overridden on this node",
                id
            );
        }
        if let Some(id) = profile
            .light_schedule
            .as_ref()
            .and_then(rhythm_core::LightScheduleAssignment::schedule_id)
        {
            anyhow::ensure!(
                schedules.contains_key(id),
                "Unknown light schedule '{}' for node '{}'",
                id,
                source
            );
        }
        let profile_patch = imported_room_profile_patch(&BackupConfigurationRoom {
            id: target.clone(),
            name: snapshot.name.clone(),
            rhythm_enabled: imported.rhythm_enabled,
            disabled: imported.disabled,
            state: RoomModeState::Active,
            room_profile: profile.clone(),
        });
        if snapshot.kind == LightNodeKind::LightDevice
            && snapshot.parent_id.is_some()
            && profile_patch.requests_nonempty_lighting_output_settings()
        {
            let app = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            anyhow::ensure!(!app.topology.attached_light_uses_parent_dispatch(target, &app.canonical_registry),
                "Individual preferences for '{}' are controlled by its room; skip this light and map its room", target);
        }
        snapshot.rhythm_enabled = imported.rhythm_enabled;
        snapshot.disabled = imported.disabled;
        snapshot.standby_enabled = imported.standby_enabled;
        snapshot.profile_settings = profile;
    }
    let schedule_configs = schedules.values().cloned().collect::<Vec<_>>();
    validate_effective_light_schedule_overrides(&snapshots, &schedule_configs, None)?;
    if let Some(imported_modes) = incoming.mode_configs {
        validate_mode_configs(&imported_modes)?;
        let mut mode_ids = HashSet::new();
        for mut mode in imported_modes {
            anyhow::ensure!(mode_ids.insert(mode.mode), "Duplicate mode configuration");
            for id in [
                &mode.active_profile_id,
                &mode.idle_profile_id,
                &mode.wake_profile_id,
                &mode.warning_profile_id,
            ]
            .into_iter()
            .flatten()
            {
                anyhow::ensure!(
                    profile_ids.contains(id),
                    "Mode references unknown profile '{}'",
                    id
                );
            }
            mode.room_defaults.retain_mut(|default| {
                if let Some(target) = mappings.get(&default.room_id) {
                    default.room_id = target.clone();
                    true
                } else {
                    warnings.insert("unmapped_mode_defaults_excluded".into());
                    false
                }
            });
            if let Some(old) = modes.iter_mut().find(|old| old.mode == mode.mode) {
                let mut defaults = old.room_defaults.clone();
                defaults.retain(|default| !targets.contains(&default.room_id));
                defaults.extend(mode.room_defaults);
                mode.room_defaults = defaults;
                *old = mode;
            } else {
                modes.push(mode);
            }
        }
    }
    // No validation failures below this boundary. Keep control paused after HA
    // transfers so review/selection and stopping the old writer happen first.
    if is_ha {
        do_light_breaker_set(state, false)?;
    }
    {
        let mut app = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        // Global and named schedule modes can advance while the review is
        // validated. Read live modes at commit; source modes are never applied.
        let schedule_configs = schedule_configs
            .into_iter()
            .map(|mut schedule| {
                schedule.active_mode = app
                    .light_schedules
                    .get(&schedule.id)
                    .map(|current| current.active_mode)
                    .unwrap_or(app.active_mode);
                schedule
            })
            .collect::<Vec<_>>();
        if let Some(runtime) = &runtime {
            for profile in profiles.values() {
                runtime.set_light_profile_config(profile.clone())?;
            }
            runtime.set_mode_configs(modes.clone())?;
            let active_profile_id =
                resolved_active_profile_id_for_mode_from_parts(&profiles, &modes, app.active_mode);
            anyhow::ensure!(
                runtime.set_light_profile(&active_profile_id),
                "Failed to activate imported mode profile"
            );
            runtime.set_power_save(incoming.profile.power_save);
            for snapshot in snapshots.iter().filter(|node| targets.contains(&node.id)) {
                let applied = runtime.apply_light_node_preferences(
                    &snapshot.id,
                    rhythm_core::runtime::handle::LightNodePreferences {
                        rhythm_enabled: snapshot.rhythm_enabled,
                        disabled: snapshot.disabled,
                        standby_enabled: snapshot.standby_enabled,
                        profile_settings: snapshot.profile_settings.clone(),
                    },
                    expected_enablement[&snapshot.id],
                )?;
                if !applied {
                    warnings.insert("newer_target_enablement_preserved".into());
                }
            }
        }
        app.replace_light_profile_configs(profiles.into_values());
        app.scenes = scenes;
        app.power_save = incoming.profile.power_save;
        app.set_mode_configs(modes);
        app.sync_active_mode_runtime_overrides();
        app.set_mode_transition_configs(incoming.profile.mode_transitions);
        app.set_light_schedule_configs(schedule_configs);
        app.room_schedule_evaluations.clear();
        if let Some(storage) = &app.storage {
            storage.save_light_profiles(&crate::storage::StoredLightProfiles::from_state(
                &app.light_profile_configs,
                &app.runtime_config,
            ))?;
            storage.save_scenes(&app.stored_scenes())?;
            storage.save_settings(&crate::storage::StoredSettings {
                power_save: app.power_save,
                light_breaker_enabled: app.light_breaker_enabled,
                light_runtime: app.light_runtime_kind.clone(),
                active_mode: app.active_mode,
                last_active_mode_cause: app.last_active_mode_cause,
                last_active_mode_transition_id: app.last_active_mode_transition_id.clone(),
                last_active_mode_change_utc_ms: app.last_active_mode_change_utc_ms,
                modes: app.mode_configs(),
                mode_transitions: app.mode_transition_configs(),
                light_schedules: app.light_schedule_configs(),
                auto_update: app.auto_update,
                update_channel: app.update_channel,
            })?;
            if let Some(runtime) = &runtime {
                storage.save_rooms(&rooms_from_engine(runtime.as_ref()))?;
            }
        }
    }
    crate::state::emit_server_event(state, crate::server_event::ServerEvent::ConfigChanged);
    crate::state::emit_server_event(state, crate::server_event::ServerEvent::NodesChanged);
    let settings: Value = serde_json::from_str(&build_lighting_settings_bundle(state)?)?;
    Ok(serde_json::to_string(&serde_json::json!({
        "settings": settings,
        "applied_nodes": mappings.len(),
        "skipped_nodes": incoming.nodes.len() - mappings.len(),
        "warnings": warnings,
    }))?)
}
