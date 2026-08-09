//! Deterministic automatic names for canonical light devices.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use log::warn;
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_devices::LightType;

use crate::canonical::identity::{CanonicalDevice, HubKey, IntegrationEndpoint};
use crate::hub::HubType;
use crate::state::{AppState, SharedState};

#[derive(Clone, Debug, PartialEq, Eq)]
struct PlannedLightName {
    canonical_id: String,
    desired_name: String,
    canonical_name_changed: bool,
    hue_endpoints: Vec<(HubKey, String)>,
}

/// The smallest canonical/topology region whose generated light names may
/// have changed after one user or discovery operation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LightNameReconciliationScope {
    device_ids: HashSet<String>,
    room_ids: HashSet<Option<String>>,
}

impl LightNameReconciliationScope {
    pub fn for_device(device_id: impl Into<String>) -> Self {
        let mut scope = Self::default();
        scope.include_device(device_id);
        scope
    }

    pub fn for_room(room_id: Option<String>) -> Self {
        let mut scope = Self::default();
        scope.include_room(room_id);
        scope
    }

    pub fn include_device(&mut self, device_id: impl Into<String>) {
        self.device_ids.insert(device_id.into());
    }

    pub fn include_room(&mut self, room_id: Option<String>) {
        self.room_ids.insert(room_id);
    }

    fn includes(&self, device_id: &str, room_id: &Option<String>) -> bool {
        self.device_ids.contains(device_id) || self.room_ids.contains(room_id)
    }
}

/// Reconcile only active light names affected by one discovery/topology event.
///
/// Callers already serialize discovery/topology mutations with the external
/// topology transaction. Hue native writes happen before the corresponding
/// local name is committed. Per-device failures are deliberately contained so
/// an unavailable bridge cannot roll back an otherwise durable room mutation.
pub fn reconcile_automatic_light_names(
    state: &SharedState,
    scope: &LightNameReconciliationScope,
) -> Result<usize> {
    let (plans, rename_hub_device) = {
        let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (
            planned_light_names(&state, scope),
            state.rename_hub_device_fn.clone(),
        )
    };

    let mut changed = 0;
    for plan in plans {
        let mut native_rename_failed = false;
        let hue_endpoints = if plan.canonical_name_changed {
            plan.hue_endpoints.as_slice()
        } else {
            &[]
        };
        for (hub_key, native_id) in hue_endpoints {
            let Some(rename_hub_device) = rename_hub_device.as_ref() else {
                warn!(
                    target: "device_naming",
                    "automatic_light_name_failed canonical_id={} hub_type={} stage=native_rename callback=unavailable",
                    plan.canonical_id,
                    hub_key.hub_type.as_str()
                );
                native_rename_failed = true;
                break;
            };
            if let Err(error) = rename_hub_device(state, hub_key, native_id, &plan.desired_name) {
                warn!(
                    target: "device_naming",
                    "automatic_light_name_failed canonical_id={} hub_type={} stage=native_rename error={}",
                    plan.canonical_id,
                    hub_key.hub_type.as_str(),
                    sanitized_error_class(&error)
                );
                native_rename_failed = true;
                break;
            }
        }
        if native_rename_failed {
            continue;
        }

        let mut state_guard = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let Some(device) = state_guard.canonical_registry.get(&plan.canonical_id) else {
            continue;
        };
        if device.is_removed()
            || device.device_type != DeviceType::Light
            || device.name == plan.desired_name
        {
            continue;
        }

        let previous_name = device.name.clone();
        state_guard
            .canonical_registry
            .get_mut(&plan.canonical_id)
            .expect("canonical device was checked above")
            .name = plan.desired_name.clone();
        if let Err(error) = crate::commands::save_authority_state(&state_guard) {
            state_guard
                .canonical_registry
                .get_mut(&plan.canonical_id)
                .expect("canonical device cannot disappear while state is locked")
                .name = previous_name;
            warn!(
                target: "device_naming",
                "automatic_light_name_failed canonical_id={} stage=canonical_persist error={}",
                plan.canonical_id,
                sanitized_error_class(&error)
            );
            continue;
        }
        changed += 1;
    }
    Ok(changed)
}

fn sanitized_error_class(error: &anyhow::Error) -> &'static str {
    let message = error.to_string().to_ascii_lowercase();
    if message.contains("timeout") {
        "timeout"
    } else if message.contains("lock") {
        "lock"
    } else if message.contains("persist") || message.contains("storage") {
        "storage"
    } else if message.contains("http") || message.contains("connection") {
        "transport"
    } else {
        "integration"
    }
}

fn planned_light_names(
    state: &AppState,
    scope: &LightNameReconciliationScope,
) -> Vec<PlannedLightName> {
    let mut candidates = state
        .canonical_registry
        .devices()
        .filter(|device| !device.is_removed() && device.device_type == DeviceType::Light)
        .map(|device| {
            let room_id = state
                .topology
                .device_parent_room_id(&device.id)
                .map(str::to_string)
                .or_else(|| device.room_id.clone());
            (device, room_id)
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|(left, left_room), (right, right_room)| {
        left_room
            .cmp(right_room)
            .then_with(|| left.created_at.cmp(&right.created_at))
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut ordinals: HashMap<Option<String>, usize> = HashMap::new();
    candidates
        .into_iter()
        .filter_map(|(device, room_id)| {
            let ordinal = ordinals.entry(room_id.clone()).or_insert(0);
            *ordinal += 1;
            if !device.has_active_endpoint() || !scope.includes(&device.id, &room_id) {
                return None;
            }
            let room_suffix = room_id
                .as_deref()
                .and_then(|room_id| state.topology.get(room_id))
                .map(|room| compact_pascal_case(&room.name))
                .filter(|room| !room.is_empty());
            let endpoint = device
                .active_endpoints()
                .find(|endpoint| endpoint.hub_key.hub_type.as_str() == HubType::HUE)
                .or_else(|| device.preferred_endpoint())
                .expect("active endpoint checked");
            let name = format_light_name(device, endpoint, *ordinal, room_suffix.as_deref());
            let hue_endpoints = device
                .active_endpoints()
                .filter(|endpoint| endpoint.hub_key.hub_type.as_str() == HubType::HUE)
                .map(|endpoint| (endpoint.hub_key.clone(), endpoint.native_id.clone()))
                .collect();
            Some(PlannedLightName {
                canonical_id: device.id.clone(),
                canonical_name_changed: device.name != name,
                desired_name: name,
                hue_endpoints,
            })
        })
        .collect()
}

fn format_light_name(
    device: &CanonicalDevice,
    endpoint: &IntegrationEndpoint,
    ordinal: usize,
    room_suffix: Option<&str>,
) -> String {
    let mut components = vec![
        brand_label(device, endpoint),
        protocol_label(endpoint).to_string(),
        color_label(device, endpoint).to_string(),
        form_factor_label(device).to_string(),
        ordinal.to_string(),
    ];
    if let Some(room_suffix) = room_suffix.filter(|name| !name.is_empty()) {
        components.push(room_suffix.to_string());
    }
    let name = components.join(" ");
    let max_bytes = if endpoint.hub_key.hub_type.as_str() == HubType::HUE {
        32
    } else {
        64
    };
    truncate_utf8_with_ellipsis(&name, max_bytes)
}

fn brand_label(device: &CanonicalDevice, endpoint: &IntegrationEndpoint) -> String {
    if endpoint.hub_key.hub_type.as_str() == HubType::HUE
        || device.manufacturer.as_deref().is_some_and(|manufacturer| {
            let manufacturer = manufacturer.to_ascii_lowercase();
            manufacturer.contains("signify") || manufacturer.contains("philips")
        })
    {
        return "Hue".to_string();
    }
    let source = device
        .manufacturer
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or(device.model.as_deref())
        .unwrap_or("Bulb");
    let abbreviation = source
        .chars()
        .filter(|character| character.is_alphanumeric())
        .take(4)
        .flat_map(char::to_uppercase)
        .collect::<String>();
    if abbreviation.is_empty() {
        "BULB".to_string()
    } else {
        abbreviation
    }
}

fn protocol_label(endpoint: &IntegrationEndpoint) -> &'static str {
    match endpoint.hub_key.hub_type.as_str() {
        HubType::HUE => "Zig",
        HubType::HUE_BLE | HubType::LOCAL_BLE => "BLE",
        HubType::HA => "HA",
        HubType::MATTER => {
            if endpoint
                .capabilities
                .as_ref()
                .and_then(|value| value.pointer("/automatic_naming/network"))
                .and_then(serde_json::Value::as_str)
                == Some("thread")
            {
                "MatThr"
            } else {
                "MatWifi"
            }
        }
        _ => "HA",
    }
}

fn color_label(device: &CanonicalDevice, endpoint: &IntegrationEndpoint) -> &'static str {
    match endpoint
        .capabilities
        .as_ref()
        .and_then(|value| value.pointer("/automatic_naming/color_kind"))
        .and_then(serde_json::Value::as_str)
    {
        Some("color") => return "Color",
        Some("white") => return "White",
        _ => {}
    }
    if let (Some(manufacturer), Some(model)) = (&device.manufacturer, &device.model) {
        if let Some(entry) = rhythm_devices::builtin_db().lookup(manufacturer, model) {
            return if entry.light_type == LightType::ExtendedColor {
                "Color"
            } else {
                "White"
            };
        }
        if let Some(entry) = rhythm_devices::builtin_db().lookup_zigbee(model) {
            return if entry.light_type == LightType::ExtendedColor {
                "Color"
            } else {
                "White"
            };
        }
    }
    let evidence = format!(
        "{} {}",
        device.model.as_deref().unwrap_or_default(),
        device.name
    )
    .to_ascii_lowercase();
    if ["color", "colour", "rgb", "extended color"]
        .iter()
        .any(|needle| evidence.contains(needle))
    {
        "Color"
    } else {
        "White"
    }
}

fn form_factor_label(device: &CanonicalDevice) -> &'static str {
    let database_name = match (&device.manufacturer, &device.model) {
        (Some(manufacturer), Some(model)) => rhythm_devices::builtin_db()
            .lookup(manufacturer, model)
            .or_else(|| rhythm_devices::builtin_db().lookup_zigbee(model))
            .map(|entry| entry.name.as_str())
            .unwrap_or_default(),
        _ => "",
    };
    let evidence = format!(
        "{} {} {}",
        database_name,
        device.model.as_deref().unwrap_or_default(),
        device.name
    )
    .to_ascii_lowercase();
    if ["downlight", "down light", "recessed", "spotlight", "gu10"]
        .iter()
        .any(|needle| evidence.contains(needle))
    {
        "Downlight"
    } else {
        "Lamp"
    }
}

fn compact_pascal_case(value: &str) -> String {
    value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| {
            let normalize_remainder = word
                .chars()
                .all(|character| !character.is_alphabetic() || character.is_uppercase())
                || word
                    .chars()
                    .all(|character| !character.is_alphabetic() || character.is_lowercase());
            let mut characters = word.chars();
            let first = characters
                .next()
                .into_iter()
                .flat_map(char::to_uppercase)
                .collect::<String>();
            let remainder = characters.collect::<String>();
            if normalize_remainder {
                format!("{}{}", first, remainder.to_lowercase())
            } else {
                format!("{}{}", first, remainder)
            }
        })
        .collect()
}

fn truncate_utf8_with_ellipsis(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    const ELLIPSIS: &str = "…";
    let available = max_bytes.saturating_sub(ELLIPSIS.len());
    let mut boundary = available.min(value.len());
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    format!("{}{}", value[..boundary].trim_end(), ELLIPSIS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::identity::{DiscoveredIdentity, HardwareId};
    use crate::canonical::registry::ResolveResult;
    use std::sync::{Arc, Mutex};

    fn add_light(
        state: &mut AppState,
        key: HubKey,
        native_id: &str,
        name: &str,
        manufacturer: &str,
        model: &str,
        created_at: u64,
    ) -> String {
        let identity = DiscoveredIdentity {
            native_id: native_id.to_string(),
            room_id: None,
            room_name: None,
            name: name.to_string(),
            device_type: DeviceType::Light,
            hardware_ids: vec![HardwareId::serial(native_id)],
            manufacturer: Some(manufacturer.to_string()),
            model: Some(model.to_string()),
        };
        match state
            .canonical_registry
            .resolve(&identity, &key, created_at)
        {
            ResolveResult::Created { canonical_id } => canonical_id,
            other => panic!("expected created device, got {other:?}"),
        }
    }

    #[test]
    fn planner_matches_owner_examples_and_roomless_contract() {
        let mut state = AppState::default();
        let room_id = state.topology.create_room("Guest Bath");
        let hue_id = add_light(
            &mut state,
            HubKey::new(HubType::new(HubType::HUE), "bridge"),
            "hue-device",
            "Hue color lamp",
            "Signify Netherlands B.V.",
            "LCT016",
            1,
        );
        state
            .canonical_registry
            .assign_room(&hue_id, Some(&room_id));
        state.topology.ensure_standalone_device(&hue_id);
        assert!(state.topology.assign_device(
            &hue_id,
            Some(&room_id),
            crate::topology::DevicePlacement::HubDefault,
        ));
        let matter_id = add_light(
            &mut state,
            HubKey::new(HubType::new(HubType::MATTER), "local"),
            "matter-device",
            "Leedarson color downlight",
            "Leedarson",
            "Color Downlight",
            2,
        );
        state
            .canonical_registry
            .assign_room(&matter_id, Some(&room_id));
        state.topology.ensure_standalone_device(&matter_id);
        assert!(state.topology.assign_device(
            &matter_id,
            Some(&room_id),
            crate::topology::DevicePlacement::HubDefault,
        ));
        let roomless_id = add_light(
            &mut state,
            HubKey::new(HubType::new(HubType::HA), "ha"),
            "light.office",
            "White lamp",
            "Acme",
            "A19",
            3,
        );

        let mut scope = LightNameReconciliationScope::default();
        for id in [&hue_id, &matter_id, &roomless_id] {
            scope.include_device(id.clone());
        }
        let plans = planned_light_names(&state, &scope);
        let names = plans
            .into_iter()
            .map(|plan| (plan.canonical_id, plan.desired_name))
            .collect::<HashMap<_, _>>();
        assert_eq!(names[&hue_id], "Hue Zig Color Lamp 1 GuestBath");
        assert_eq!(
            names[&matter_id],
            "LEED MatWifi Color Downlight 2 GuestBath"
        );
        assert_eq!(names[&roomless_id], "ACME HA White Lamp 1");
    }

    #[test]
    fn planner_is_deterministic_and_understands_future_matter_thread_metadata() {
        let mut state = AppState::default();
        let id = add_light(
            &mut state,
            HubKey::new(HubType::new(HubType::MATTER), "local"),
            "matter-thread",
            "Bulb",
            "Vendor",
            "Plain",
            7,
        );
        state.canonical_registry.get_mut(&id).unwrap().endpoints[0].capabilities =
            Some(serde_json::json!({
                "automatic_naming": {
                    "network": "thread",
                    "color_kind": "color"
                }
            }));

        let scope = LightNameReconciliationScope::for_device(id);
        let first = planned_light_names(&state, &scope);
        let second = planned_light_names(&state, &scope);
        assert_eq!(first, second);
        assert_eq!(first[0].desired_name, "VEND MatThr Color Lamp 1");
    }

    #[test]
    fn sensors_and_removed_or_inactive_lights_are_excluded() {
        let mut state = AppState::default();
        let light_id = add_light(
            &mut state,
            HubKey::new(HubType::new(HubType::HUE_BLE), "local"),
            "inactive",
            "Lamp",
            "Signify",
            "LCT016",
            1,
        );
        state
            .canonical_registry
            .get_mut(&light_id)
            .unwrap()
            .endpoints[0]
            .active = false;

        let scope = LightNameReconciliationScope::for_device(light_id);
        assert!(planned_light_names(&state, &scope).is_empty());
    }

    #[test]
    fn inactive_lights_reserve_their_stable_room_ordinal_and_hue_names_are_bounded() {
        let mut state = AppState::default();
        let room_id = state
            .topology
            .create_room("An Exceptionally Long Master Bedroom");
        let inactive_id = add_light(
            &mut state,
            HubKey::new(HubType::new(HubType::HA), "ha"),
            "light.inactive",
            "Lamp",
            "Acme",
            "A19",
            1,
        );
        let active_id = add_light(
            &mut state,
            HubKey::new(HubType::new(HubType::HUE), "bridge"),
            "hue-active",
            "Hue color lamp",
            "Signify Netherlands B.V.",
            "LCT016",
            2,
        );
        for id in [&inactive_id, &active_id] {
            state.canonical_registry.assign_room(id, Some(&room_id));
            state.topology.ensure_standalone_device(id);
            assert!(state.topology.assign_device(
                id,
                Some(&room_id),
                crate::topology::DevicePlacement::HubDefault,
            ));
        }
        state
            .canonical_registry
            .get_mut(&inactive_id)
            .unwrap()
            .endpoints[0]
            .active = false;

        let scope = LightNameReconciliationScope::for_room(Some(room_id));
        let plans = planned_light_names(&state, &scope);
        assert_eq!(plans.len(), 1);
        assert!(plans[0].desired_name.starts_with("Hue Zig Color Lamp 2 "));
        assert!(plans[0].desired_name.len() <= 32);
        assert!(plans[0].desired_name.ends_with('…'));
    }

    #[test]
    fn reconciliation_updates_hue_first_then_tracks_room_rename() {
        let shared = Arc::new(Mutex::new(AppState::default()));
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let (canonical_id, room_id) = {
            let mut state = shared.lock().unwrap();
            let room_id = state.topology.create_room("Guest Bath");
            let canonical_id = add_light(
                &mut state,
                HubKey::new(HubType::new(HubType::HUE), "bridge"),
                "hue-device",
                "Original Hue name",
                "Signify Netherlands B.V.",
                "LCT016",
                1,
            );
            state
                .canonical_registry
                .assign_room(&canonical_id, Some(&room_id));
            state.topology.ensure_standalone_device(&canonical_id);
            assert!(state.topology.assign_device(
                &canonical_id,
                Some(&room_id),
                crate::topology::DevicePlacement::HubDefault,
            ));
            state.rename_hub_device_fn = Some(Arc::new({
                let calls = calls.clone();
                move |_, _, _, name| {
                    calls.lock().unwrap().push(name.to_string());
                    Ok(())
                }
            }));
            (canonical_id, room_id)
        };

        let device_scope = LightNameReconciliationScope::for_device(canonical_id.clone());
        assert_eq!(
            reconcile_automatic_light_names(&shared, &device_scope).unwrap(),
            1
        );
        assert_eq!(
            shared
                .lock()
                .unwrap()
                .canonical_registry
                .get(&canonical_id)
                .unwrap()
                .name,
            "Hue Zig Color Lamp 1 GuestBath"
        );
        {
            let mut state = shared.lock().unwrap();
            assert!(state.topology.rename_room(&room_id, "Master Bedroom"));
        }
        let room_scope = LightNameReconciliationScope::for_room(Some(room_id));
        assert_eq!(
            reconcile_automatic_light_names(&shared, &room_scope).unwrap(),
            1
        );
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            [
                "Hue Zig Color Lamp 1 GuestBath",
                "Hue Zig Color Lamp 1 MasterBe…"
            ]
        );
    }

    #[test]
    fn failed_hue_native_rename_does_not_commit_canonical_name() {
        let shared = Arc::new(Mutex::new(AppState::default()));
        let canonical_id = {
            let mut state = shared.lock().unwrap();
            let canonical_id = add_light(
                &mut state,
                HubKey::new(HubType::new(HubType::HUE), "bridge"),
                "hue-device",
                "Original Hue name",
                "Signify Netherlands B.V.",
                "LCT016",
                1,
            );
            state.rename_hub_device_fn = Some(Arc::new(|_, _, _, _| {
                anyhow::bail!("private bridge response")
            }));
            canonical_id
        };

        let scope = LightNameReconciliationScope::for_device(canonical_id.clone());
        assert_eq!(reconcile_automatic_light_names(&shared, &scope).unwrap(), 0);
        assert_eq!(
            shared
                .lock()
                .unwrap()
                .canonical_registry
                .get(&canonical_id)
                .unwrap()
                .name,
            "Original Hue name"
        );
    }

    #[test]
    fn scoped_reconciliation_never_renames_an_unrelated_hue_light() {
        let shared = Arc::new(Mutex::new(AppState::default()));
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let (target_id, unrelated_id) = {
            let mut state = shared.lock().unwrap();
            let target_id = add_light(
                &mut state,
                HubKey::new(HubType::new(HubType::HUE), "bridge"),
                "hue-target",
                "Target source name",
                "Signify Netherlands B.V.",
                "LCT016",
                1,
            );
            let unrelated_id = add_light(
                &mut state,
                HubKey::new(HubType::new(HubType::HUE), "bridge"),
                "hue-unrelated",
                "Unrelated source name",
                "Signify Netherlands B.V.",
                "LCT016",
                2,
            );
            state.rename_hub_device_fn = Some(Arc::new({
                let calls = calls.clone();
                move |_, _, native_id, _| {
                    calls.lock().unwrap().push(native_id.to_string());
                    Ok(())
                }
            }));
            (target_id, unrelated_id)
        };

        let scope = LightNameReconciliationScope::for_device(target_id.clone());
        assert_eq!(reconcile_automatic_light_names(&shared, &scope).unwrap(), 1);
        let state = shared.lock().unwrap();
        assert_eq!(
            state.canonical_registry.get(&target_id).unwrap().name,
            "Hue Zig Color Lamp 1"
        );
        assert_eq!(
            state.canonical_registry.get(&unrelated_id).unwrap().name,
            "Unrelated source name"
        );
        assert_eq!(calls.lock().unwrap().as_slice(), ["hue-target"]);
    }
}
