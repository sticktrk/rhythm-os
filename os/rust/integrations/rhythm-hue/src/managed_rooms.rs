//! Reconcile hidden Hue rooms from Rhythm's authoritative room assignments.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::storage::Storage;

use crate::ownership::{
    persist_controller_ownership, HueControllerOwnership, HueManagedRoom, HueOwnershipPhase,
};
use crate::transport::{HueRoomDefinition, HueTransport};

pub const DEFAULT_MANAGED_HUE_ROOM_ARCHETYPE: &str = "living_room";
const MAX_MANAGED_HUE_ROOM_NAME_BYTES: usize = 32;
const MANAGED_HUE_ROOM_NAME_PREFIX: &str = "Rhythm · ";

/// Return a bounded, human-readable native name for one managed Hue room.
///
/// Explicit ownership remains in the per-bridge manifest. This name is only
/// presentation metadata for Hue surfaces and must never be used as identity.
pub fn managed_room_projection_name(room_name: &str) -> String {
    let normalized = room_name.split_whitespace().collect::<Vec<_>>().join(" ");
    let normalized = if normalized.is_empty() {
        "Room"
    } else {
        normalized.as_str()
    };
    let available = MAX_MANAGED_HUE_ROOM_NAME_BYTES - MANAGED_HUE_ROOM_NAME_PREFIX.len();
    if normalized.len() <= available {
        return format!("{MANAGED_HUE_ROOM_NAME_PREFIX}{normalized}");
    }

    const ELLIPSIS: &str = "…";
    let content_bytes = available - ELLIPSIS.len();
    let mut truncated = String::new();
    for character in normalized.chars() {
        if truncated.len() + character.len_utf8() > content_bytes {
            break;
        }
        truncated.push(character);
    }

    format!(
        "{MANAGED_HUE_ROOM_NAME_PREFIX}{}{ELLIPSIS}",
        truncated.trim_end()
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesiredHueRoom {
    pub rhythm_room_id: String,
    pub name: String,
    pub archetype: String,
    /// Hue V2 device IDs for light devices assigned to this Rhythm room.
    pub device_ids: Vec<String>,
}

impl DesiredHueRoom {
    pub fn new(
        rhythm_room_id: impl Into<String>,
        name: impl Into<String>,
        device_ids: Vec<String>,
    ) -> Self {
        Self {
            rhythm_room_id: rhythm_room_id.into(),
            name: name.into(),
            archetype: DEFAULT_MANAGED_HUE_ROOM_ARCHETYPE.to_string(),
            device_ids,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedHueRoom {
    pub hue_room_id: String,
    pub grouped_light_id: Option<String>,
    pub name: String,
    pub archetype: String,
    pub device_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HueManagedRoomOperation {
    /// Remove controlled bulbs from the wrong Hue room or add them to their
    /// mapped target while retaining non-light child devices.
    ReconcileMembership {
        hue_room_id: String,
        device_ids: Vec<String>,
        removal_only: bool,
    },
    Create {
        desired: DesiredHueRoom,
    },
    AdoptRecovered {
        rhythm_room_id: String,
        hue_room_id: String,
        grouped_light_id: String,
    },
    Rename {
        rhythm_room_id: String,
        hue_room_id: String,
        name: String,
    },
    Update {
        rhythm_room_id: String,
        hue_room_id: String,
        definition: HueRoomDefinition,
    },
    Delete {
        rhythm_room_id: String,
        hue_room_id: String,
    },
    ForgetMissing {
        rhythm_room_id: String,
    },
}

fn sorted_unique(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values.dedup();
    values
}

fn parse_rooms(payload: &serde_json::Value) -> Result<Vec<ObservedHueRoom>> {
    let rooms = payload
        .get("data")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("Hue room response has no data array"))?;
    let mut parsed = rooms
        .iter()
        .map(|room| {
            let hue_room_id = room
                .get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("Hue room response contains a room without an ID"))?
                .to_string();
            let grouped_light_id = room
                .get("services")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .find(|service| {
                    service.get("rtype").and_then(serde_json::Value::as_str)
                        == Some("grouped_light")
                })
                .and_then(|service| service.get("rid"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            let name = room
                .pointer("/metadata/name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let archetype = room
                .pointer("/metadata/archetype")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let device_ids = room
                .get("children")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter(|child| {
                    child.get("rtype").and_then(serde_json::Value::as_str) == Some("device")
                })
                .filter_map(|child| child.get("rid").and_then(serde_json::Value::as_str))
                .map(str::to_string)
                .collect::<Vec<_>>();
            Ok(ObservedHueRoom {
                hue_room_id,
                grouped_light_id,
                name,
                archetype,
                device_ids: sorted_unique(device_ids),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    parsed.sort_by(|left, right| left.hue_room_id.cmp(&right.hue_room_id));
    Ok(parsed)
}

pub fn observe_hue_rooms<H: HueTransport + ?Sized>(
    transport: &H,
    username: &str,
) -> Result<Vec<ObservedHueRoom>> {
    parse_rooms(&transport.get_resources(username, "room")?)
}

fn validate_desired(
    desired: &[DesiredHueRoom],
    controlled_device_ids: &BTreeSet<String>,
) -> Result<BTreeMap<String, DesiredHueRoom>> {
    let mut rooms = BTreeMap::new();
    let mut assigned = BTreeSet::new();
    for desired in desired {
        if desired.rhythm_room_id.trim().is_empty()
            || desired.name.trim().is_empty()
            || desired.archetype.trim().is_empty()
        {
            anyhow::bail!("Managed Hue rooms require non-empty Rhythm ID, name, and archetype");
        }
        let mut normalized = desired.clone();
        normalized.device_ids = sorted_unique(normalized.device_ids);
        if normalized.device_ids.is_empty() {
            anyhow::bail!("A managed Hue room must contain at least one controlled Hue bulb");
        }
        for device_id in &normalized.device_ids {
            if !controlled_device_ids.contains(device_id) {
                anyhow::bail!(
                    "A Hue room member is not in the authoritative controlled-device set"
                );
            }
            if !assigned.insert(device_id.clone()) {
                anyhow::bail!("A Hue device is assigned to more than one Rhythm room");
            }
        }
        if rooms
            .insert(normalized.rhythm_room_id.clone(), normalized)
            .is_some()
        {
            anyhow::bail!("Duplicate Rhythm room ID in Hue reconciliation input");
        }
    }
    Ok(rooms)
}

/// Return one deterministic operation. Callers re-observe after every write;
/// this keeps recovery idempotent and makes membership removal precede addition.
pub fn next_managed_room_operation(
    state: &HueControllerOwnership,
    desired: &[DesiredHueRoom],
    controlled_device_ids: &BTreeSet<String>,
    observed: &[ObservedHueRoom],
) -> Result<Option<HueManagedRoomOperation>> {
    if state.phase != HueOwnershipPhase::Active {
        anyhow::bail!("Hue room reconciliation requires an active ownership manifest");
    }
    let desired = validate_desired(desired, controlled_device_ids)?;
    let observed_by_id = observed
        .iter()
        .map(|room| (room.hue_room_id.as_str(), room))
        .collect::<BTreeMap<_, _>>();

    // A mapping with no desired Rhythm room is the only kind of room this
    // reconciler may delete. Unmapped Hue rooms are only stripped of controlled
    // bulb membership; their identity is never guessed from a name.
    for (rhythm_room_id, managed) in state.managed_rooms() {
        if !desired.contains_key(rhythm_room_id) {
            return Ok(Some(
                if observed_by_id.contains_key(managed.hue_room_id.as_str()) {
                    HueManagedRoomOperation::Delete {
                        rhythm_room_id: rhythm_room_id.clone(),
                        hue_room_id: managed.hue_room_id.clone(),
                    }
                } else {
                    HueManagedRoomOperation::ForgetMissing {
                        rhythm_room_id: rhythm_room_id.clone(),
                    }
                },
            ));
        } else if !observed_by_id.contains_key(managed.hue_room_id.as_str()) {
            // Retire the stale receipt before creating a replacement. If the
            // process dies after the remote create but before its new receipt
            // is durable, the next pass can safely adopt that unmapped room.
            return Ok(Some(HueManagedRoomOperation::ForgetMissing {
                rhythm_room_id: rhythm_room_id.clone(),
            }));
        }
    }

    let mapped_hue_room_ids = state.managed_room_ids();
    for (rhythm_room_id, desired) in &desired {
        if state.managed_rooms().contains_key(rhythm_room_id) {
            continue;
        }
        let mut equivalent = observed
            .iter()
            .filter(|room| !mapped_hue_room_ids.contains(&room.hue_room_id))
            .filter(|room| {
                room.name == desired.name
                    && room.archetype == desired.archetype
                    && room.device_ids == desired.device_ids
            })
            .filter_map(|room| {
                room.grouped_light_id
                    .as_ref()
                    .map(|grouped_light_id| (room.hue_room_id.clone(), grouped_light_id.clone()))
            })
            .collect::<Vec<_>>();
        equivalent.sort();
        match equivalent.as_slice() {
            [] => {}
            [(hue_room_id, grouped_light_id)] => {
                return Ok(Some(HueManagedRoomOperation::AdoptRecovered {
                    rhythm_room_id: rhythm_room_id.clone(),
                    hue_room_id: hue_room_id.clone(),
                    grouped_light_id: grouped_light_id.clone(),
                }));
            }
            _ => anyhow::bail!(
                "Multiple equivalent Hue rooms could satisfy one Rhythm room; refusing ambiguous recovery"
            ),
        }
    }

    let desired_room_for_device = desired
        .iter()
        .flat_map(|(rhythm_room_id, room)| {
            room.device_ids
                .iter()
                .map(move |device_id| (device_id.as_str(), rhythm_room_id.as_str()))
        })
        .collect::<BTreeMap<_, _>>();

    let expected_hue_room_for_device = desired_room_for_device
        .iter()
        .filter_map(|(device_id, rhythm_room_id)| {
            state
                .managed_rooms()
                .get(*rhythm_room_id)
                .map(|managed| (*device_id, managed.hue_room_id.as_str()))
        })
        .collect::<BTreeMap<_, _>>();

    let mut membership_changes = Vec::new();
    for room in observed {
        let mut expected = room
            .device_ids
            .iter()
            .filter(|device_id| !controlled_device_ids.contains(device_id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        expected.extend(
            controlled_device_ids
                .iter()
                .filter(|device_id| {
                    expected_hue_room_for_device.get(device_id.as_str())
                        == Some(&room.hue_room_id.as_str())
                })
                .cloned(),
        );
        expected = sorted_unique(expected);
        if expected != room.device_ids {
            let current = room.device_ids.iter().collect::<BTreeSet<_>>();
            let next = expected.iter().collect::<BTreeSet<_>>();
            let removal_only = next.is_subset(&current);
            membership_changes.push(HueManagedRoomOperation::ReconcileMembership {
                hue_room_id: room.hue_room_id.clone(),
                device_ids: expected,
                removal_only,
            });
        }
    }
    membership_changes.sort_by_key(|operation| match operation {
        HueManagedRoomOperation::ReconcileMembership {
            removal_only: true,
            hue_room_id,
            ..
        } => (0, hue_room_id.clone()),
        HueManagedRoomOperation::ReconcileMembership { hue_room_id, .. } => {
            (1, hue_room_id.clone())
        }
        _ => unreachable!(),
    });
    if let Some(operation) = membership_changes.into_iter().next() {
        return Ok(Some(operation));
    }

    for (rhythm_room_id, desired) in &desired {
        let Some(managed) = state.managed_rooms().get(rhythm_room_id) else {
            return Ok(Some(HueManagedRoomOperation::Create {
                desired: desired.clone(),
            }));
        };
        let Some(current) = observed_by_id.get(managed.hue_room_id.as_str()) else {
            return Ok(Some(HueManagedRoomOperation::ForgetMissing {
                rhythm_room_id: rhythm_room_id.clone(),
            }));
        };
        let expected_children = current.device_ids.clone();
        if current.archetype != desired.archetype {
            return Ok(Some(HueManagedRoomOperation::Update {
                rhythm_room_id: rhythm_room_id.clone(),
                hue_room_id: managed.hue_room_id.clone(),
                definition: HueRoomDefinition {
                    name: desired.name.clone(),
                    archetype: desired.archetype.clone(),
                    device_ids: expected_children,
                },
            }));
        }
        if current.name != desired.name {
            return Ok(Some(HueManagedRoomOperation::Rename {
                rhythm_room_id: rhythm_room_id.clone(),
                hue_room_id: managed.hue_room_id.clone(),
                name: desired.name.clone(),
            }));
        }
        if current.grouped_light_id.as_deref() != Some(managed.grouped_light_id.as_str()) {
            anyhow::bail!("A managed Hue room changed grouped-light identity");
        }
    }

    Ok(None)
}

fn execute_managed_room_operation<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    _key: &HubKey,
    state: &mut HueControllerOwnership,
    transport: &H,
    username: &str,
    operation: HueManagedRoomOperation,
) -> Result<()> {
    if let HueManagedRoomOperation::Delete { hue_room_id, .. } = &operation {
        let scenes = transport.get_resources(username, "scene")?;
        let scenes = scenes
            .get("data")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| anyhow::anyhow!("Hue scene response has no data array"))?;
        let managed_scene_ids = state
            .managed_scenes()
            .values()
            .map(|scene| scene.hue_scene_id.as_str())
            .collect::<BTreeSet<_>>();
        let has_native_scene = scenes.iter().any(|scene| {
            scene
                .pointer("/group/rtype")
                .and_then(serde_json::Value::as_str)
                == Some("room")
                && scene
                    .pointer("/group/rid")
                    .and_then(serde_json::Value::as_str)
                    == Some(hue_room_id.as_str())
                && !scene
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|scene_id| managed_scene_ids.contains(scene_id))
        });
        if has_native_scene {
            anyhow::bail!(
                "Hue-authored scenes must be removed or moved before Rhythm can delete their managed room"
            );
        }
    }
    let scene_room_id = match &operation {
        HueManagedRoomOperation::ReconcileMembership { hue_room_id, .. } => state
            .managed_rooms()
            .values()
            .find(|room| room.hue_room_id == *hue_room_id)
            .map(|room| room.rhythm_room_id.clone()),
        HueManagedRoomOperation::Update { rhythm_room_id, .. }
        | HueManagedRoomOperation::Delete { rhythm_room_id, .. }
        | HueManagedRoomOperation::ForgetMissing { rhythm_room_id } => Some(rhythm_room_id.clone()),
        HueManagedRoomOperation::Create { desired } => state
            .managed_rooms()
            .contains_key(&desired.rhythm_room_id)
            .then(|| desired.rhythm_room_id.clone()),
        HueManagedRoomOperation::AdoptRecovered { .. } | HueManagedRoomOperation::Rename { .. } => {
            None
        }
    };
    if let Some(rhythm_room_id) = scene_room_id {
        crate::managed_scenes::delete_managed_scenes_for_room(
            storage,
            state,
            transport,
            username,
            &rhythm_room_id,
        )?;
    }

    match operation {
        HueManagedRoomOperation::ReconcileMembership {
            hue_room_id,
            device_ids,
            ..
        } => transport.update_room_children(username, &hue_room_id, &device_ids),
        HueManagedRoomOperation::Create { desired } => {
            let definition = HueRoomDefinition {
                name: desired.name,
                archetype: desired.archetype,
                device_ids: desired.device_ids,
            };
            let created = transport.create_room(username, &definition)?;
            let observed = observe_hue_rooms(transport, username)?;
            let created = observed
                .iter()
                .find(|room| room.hue_room_id == created.room_id)
                .ok_or_else(|| {
                    anyhow::anyhow!("Hue bridge did not expose the newly created room")
                })?;
            let grouped_light_id = created.grouped_light_id.clone().ok_or_else(|| {
                anyhow::anyhow!("Newly created Hue room has no grouped_light service")
            })?;
            state.record_managed_room(HueManagedRoom {
                rhythm_room_id: desired.rhythm_room_id,
                hue_room_id: created.hue_room_id.clone(),
                grouped_light_id,
            })?;
            persist_controller_ownership(storage, state)
        }
        HueManagedRoomOperation::AdoptRecovered {
            rhythm_room_id,
            hue_room_id,
            grouped_light_id,
        } => {
            state.record_managed_room(HueManagedRoom {
                rhythm_room_id,
                hue_room_id,
                grouped_light_id,
            })?;
            persist_controller_ownership(storage, state)
        }
        HueManagedRoomOperation::Rename {
            hue_room_id, name, ..
        } => transport.rename_room(username, &hue_room_id, &name),
        HueManagedRoomOperation::Update {
            hue_room_id,
            definition,
            ..
        } => transport.update_room(username, &hue_room_id, &definition),
        HueManagedRoomOperation::Delete {
            rhythm_room_id,
            hue_room_id,
        } => {
            transport.delete_room(username, &hue_room_id)?;
            state.remove_managed_room(&rhythm_room_id)?;
            persist_controller_ownership(storage, state)
        }
        HueManagedRoomOperation::ForgetMissing { rhythm_room_id } => {
            state.remove_managed_room(&rhythm_room_id)?;
            persist_controller_ownership(storage, state)
        }
    }
    .context("Hue managed-room operation failed")
}

/// Reconcile all controlled Hue bulbs. A controlled bulb omitted from every
/// desired room is explicitly standalone and is removed from all Hue rooms.
pub fn reconcile_managed_rooms<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    key: &HubKey,
    state: &mut HueControllerOwnership,
    transport: &H,
    username: &str,
    desired: &[DesiredHueRoom],
    controlled_device_ids: &BTreeSet<String>,
) -> Result<()> {
    validate_desired(desired, controlled_device_ids)?;
    let mut previous_operation: Option<HueManagedRoomOperation> = None;
    loop {
        let observed = observe_hue_rooms(transport, username)?;
        let Some(operation) =
            next_managed_room_operation(state, desired, controlled_device_ids, &observed)?
        else {
            return Ok(());
        };
        if previous_operation.as_ref() == Some(&operation) {
            anyhow::bail!("A managed Hue room mutation failed read-back verification");
        }
        previous_operation = Some(operation.clone());
        execute_managed_room_operation(storage, key, state, transport, username, operation)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ownership::{acquire_authoritative_control, load_controller_ownership};
    use crate::test_support::{HueTransportCall, SpyHueTransport};
    use rhythm_os::discovery::HubDiscovery;
    use rhythm_os::hub::HubType;
    use rhythm_os::storage::FileStorage;
    use serde_json::json;
    use std::sync::Arc;

    struct TempStorage {
        path: std::path::PathBuf,
        storage: FileStorage,
    }

    impl TempStorage {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "rhythm-hue-managed-rooms-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            let storage = FileStorage::new(path.to_str().unwrap()).unwrap();
            Self { path, storage }
        }
    }

    impl Drop for TempStorage {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn key() -> HubKey {
        HubKey::new(HubType::new(HubType::HUE), "192.0.2.10")
    }

    #[test]
    fn managed_room_projection_name_is_human_readable_and_bounded() {
        assert_eq!(
            managed_room_projection_name("  Living   Room  "),
            "Rhythm · Living Room"
        );
        assert_eq!(managed_room_projection_name("  \t "), "Rhythm · Room");

        let long_ascii = managed_room_projection_name(
            "A very long downstairs family room with several reading lights",
        );
        assert!(long_ascii.starts_with("Rhythm · A very long"));
        assert!(long_ascii.ends_with('…'));
        assert!(long_ascii.len() <= MAX_MANAGED_HUE_ROOM_NAME_BYTES);

        let multibyte = managed_room_projection_name("リビングルームと読書コーナー");
        assert!(multibyte.starts_with("Rhythm · リビング"));
        assert!(multibyte.ends_with('…'));
        assert!(multibyte.len() <= MAX_MANAGED_HUE_ROOM_NAME_BYTES);
        assert!(multibyte.is_char_boundary(multibyte.len()));
    }

    #[test]
    fn existing_hash_named_room_is_renamed_in_place() {
        let temp = TempStorage::new("readable-rename");
        let spy = SpyHueTransport::new();
        let mut state = active_state(&spy, &temp.storage);
        let controlled = BTreeSet::from(["bulb".to_string()]);

        reconcile_managed_rooms(
            &temp.storage,
            &key(),
            &mut state,
            &spy,
            "user",
            &[DesiredHueRoom::new(
                "rhythm-a",
                "Rhythm 0123456789abcdef",
                vec!["bulb".to_string()],
            )],
            &controlled,
        )
        .unwrap();
        let hue_room_id = state.managed_rooms()["rhythm-a"].hue_room_id.clone();
        let grouped_light_id = state.managed_rooms()["rhythm-a"].grouped_light_id.clone();
        spy.reset();

        reconcile_managed_rooms(
            &temp.storage,
            &key(),
            &mut state,
            &spy,
            "user",
            &[DesiredHueRoom::new(
                "rhythm-a",
                "Rhythm · Living Room",
                vec!["bulb".to_string()],
            )],
            &controlled,
        )
        .unwrap();

        assert_eq!(state.managed_rooms()["rhythm-a"].hue_room_id, hue_room_id);
        assert_eq!(
            state.managed_rooms()["rhythm-a"].grouped_light_id,
            grouped_light_id
        );
        let mutations = spy
            .calls()
            .into_iter()
            .filter(|call| {
                matches!(
                    call,
                    HueTransportCall::CreateRoom { .. }
                        | HueTransportCall::UpdateRoom { .. }
                        | HueTransportCall::RenameRoom { .. }
                        | HueTransportCall::DeleteRoom { .. }
                        | HueTransportCall::UpdateRoomChildren { .. }
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            mutations,
            vec![HueTransportCall::RenameRoom {
                room_id: hue_room_id,
                name: "Rhythm · Living Room".to_string(),
            }]
        );
    }

    fn active_state(spy: &SpyHueTransport, storage: &FileStorage) -> HueControllerOwnership {
        for (resource_type, data) in [
            ("bridge", json!([{"id": "bridge-rooms"}])),
            ("device", json!([{"id": "bulb"}, {"id": "switch"}])),
            ("light", json!([{"id": "light"}])),
            ("behavior_instance", json!([])),
            ("room", json!([])),
            ("zone", json!([])),
            ("scene", json!([])),
            ("smart_scene", json!([])),
        ] {
            spy.set_resource_response(resource_type, json!({"data": data, "errors": []}));
        }
        spy.set_v1_response("rules", json!({}));
        spy.set_v1_response("schedules", json!({}));
        acquire_authoritative_control(storage, &key(), spy, "user").unwrap()
    }

    #[test]
    fn creates_hidden_room_and_moves_bulb_by_removing_before_adding() {
        let temp = TempStorage::new("move");
        let spy = SpyHueTransport::new();
        let mut state = active_state(&spy, &temp.storage);
        let controlled = BTreeSet::from(["bulb".to_string()]);
        reconcile_managed_rooms(
            &temp.storage,
            &key(),
            &mut state,
            &spy,
            "user",
            &[DesiredHueRoom::new(
                "rhythm-a",
                "Nook",
                vec!["bulb".to_string()],
            )],
            &controlled,
        )
        .unwrap();
        let first_room_id = state.managed_rooms()["rhythm-a"].hue_room_id.clone();
        spy.reset();

        reconcile_managed_rooms(
            &temp.storage,
            &key(),
            &mut state,
            &spy,
            "user",
            &[DesiredHueRoom::new(
                "rhythm-b",
                "Office",
                vec!["bulb".to_string()],
            )],
            &controlled,
        )
        .unwrap();

        let calls = spy.calls();
        let remove_index = calls
            .iter()
            .position(|call| {
                matches!(
                    call,
                    HueTransportCall::DeleteRoom { room_id }
                        if room_id == &first_room_id
                )
            })
            .unwrap();
        let create_index = calls
            .iter()
            .position(|call| matches!(call, HueTransportCall::CreateRoom { .. }))
            .unwrap();
        assert!(remove_index < create_index);
        assert_eq!(state.managed_rooms().len(), 1);
        assert!(state.managed_rooms().contains_key("rhythm-b"));
    }

    #[test]
    fn refuses_to_delete_managed_room_with_hue_authored_scene() {
        let temp = TempStorage::new("native-scene-delete-fence");
        let spy = SpyHueTransport::new();
        let mut state = active_state(&spy, &temp.storage);
        let controlled = BTreeSet::from(["bulb".to_string()]);
        reconcile_managed_rooms(
            &temp.storage,
            &key(),
            &mut state,
            &spy,
            "user",
            &[DesiredHueRoom::new(
                "rhythm-room",
                "Nook",
                vec!["bulb".to_string()],
            )],
            &controlled,
        )
        .unwrap();
        let managed_room_id = state.managed_rooms()["rhythm-room"].hue_room_id.clone();
        spy.set_resource_response(
            "scene",
            json!({"data": [{
                "id": "hue-native-scene",
                "group": {"rtype": "room", "rid": managed_room_id}
            }], "errors": []}),
        );
        spy.reset();

        let error = reconcile_managed_rooms(
            &temp.storage,
            &key(),
            &mut state,
            &spy,
            "user",
            &[],
            &controlled,
        )
        .unwrap_err();

        assert!(error.to_string().contains("Hue-authored scenes"));
        assert!(state.managed_rooms().contains_key("rhythm-room"));
        assert!(!spy
            .calls()
            .iter()
            .any(|call| matches!(call, HueTransportCall::DeleteRoom { .. })));
        assert!(!spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::DeleteResource { resource_type, .. }
                if resource_type == "scene"
        )));
    }

    #[test]
    fn standalone_bulb_is_removed_from_unmanaged_room_without_deleting_it() {
        let temp = TempStorage::new("standalone");
        let spy = SpyHueTransport::new();
        let mut state = active_state(&spy, &temp.storage);
        spy.set_resource_response(
            "room",
            json!({"data": [{
                "id": "unmanaged",
                "children": [
                    {"rid": "bulb", "rtype": "device"},
                    {"rid": "switch", "rtype": "device"}
                ],
                "metadata": {"name": "External", "archetype": "other"},
                "services": [{"rid": "grouped-unmanaged", "rtype": "grouped_light"}]
            }], "errors": []}),
        );
        reconcile_managed_rooms(
            &temp.storage,
            &key(),
            &mut state,
            &spy,
            "user",
            &[],
            &BTreeSet::from(["bulb".to_string()]),
        )
        .unwrap();

        let rooms = observe_hue_rooms(&spy, "user").unwrap();
        assert_eq!(rooms.len(), 1);
        assert_eq!(rooms[0].hue_room_id, "unmanaged");
        assert_eq!(rooms[0].device_ids, vec!["switch"]);
        assert!(!spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::DeleteRoom { room_id } if room_id == "unmanaged"
        )));
    }

    #[test]
    fn membership_reconcile_fails_boundedly_when_readback_does_not_change() {
        let temp = TempStorage::new("membership-readback");
        let spy = SpyHueTransport::new();
        let mut state = active_state(&spy, &temp.storage);
        spy.set_resource_response(
            "room",
            json!({"data": [{
                "id": "unmanaged",
                "children": [{"rid": "bulb", "rtype": "device"}],
                "metadata": {"name": "External", "archetype": "other"},
                "services": [{"rid": "grouped-unmanaged", "rtype": "grouped_light"}]
            }], "errors": []}),
        );
        spy.set_ignore_resource_mutations(true);

        let error = reconcile_managed_rooms(
            &temp.storage,
            &key(),
            &mut state,
            &spy,
            "user",
            &[],
            &BTreeSet::from(["bulb".to_string()]),
        )
        .expect_err("unobserved membership must fail rather than retry forever");

        assert!(error.to_string().contains("read-back verification"));
        assert_eq!(
            spy.calls()
                .iter()
                .filter(|call| matches!(call, HueTransportCall::UpdateRoomChildren { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn managed_mapping_is_persisted_by_stable_bridge_identity() {
        let temp = TempStorage::new("persist");
        let spy = SpyHueTransport::new();
        let mut state = active_state(&spy, &temp.storage);
        reconcile_managed_rooms(
            &temp.storage,
            &key(),
            &mut state,
            &spy,
            "user",
            &[DesiredHueRoom::new(
                "rhythm-a",
                "Nook",
                vec!["bulb".to_string()],
            )],
            &BTreeSet::from(["bulb".to_string()]),
        )
        .unwrap();
        let reloaded = load_controller_ownership(&temp.storage, "bridge-rooms")
            .unwrap()
            .unwrap();
        assert_eq!(reloaded.managed_rooms(), state.managed_rooms());
    }

    #[test]
    fn persisted_discovery_scope_observes_new_managed_rooms_without_reconstruction() {
        let temp = TempStorage::new("dynamic-discovery");
        let spy = Arc::new(SpyHueTransport::new());
        let mut state = active_state(spy.as_ref(), &temp.storage);
        let storage: Arc<dyn Storage> =
            Arc::new(FileStorage::new(temp.path.to_str().unwrap()).unwrap());
        let discovery = crate::discovery::HueDiscovery::new_with_ownership_storage(
            spy.clone(),
            "user".to_string(),
            storage,
        )
        .unwrap();
        assert!(discovery.discover_rooms().unwrap().is_empty());

        reconcile_managed_rooms(
            &temp.storage,
            &key(),
            &mut state,
            &spy,
            "user",
            &[DesiredHueRoom::new(
                "rhythm-a",
                "Nook",
                vec!["bulb".to_string()],
            )],
            &BTreeSet::from(["bulb".to_string()]),
        )
        .unwrap();

        let rooms = discovery.discover_rooms().unwrap();
        assert_eq!(rooms.len(), 1);
        assert_eq!(rooms[0].id, state.managed_rooms()["rhythm-a"].hue_room_id);
    }

    #[test]
    fn equivalent_unmapped_room_is_adopted_after_interrupted_create() {
        let temp = TempStorage::new("adopt-create");
        let spy = SpyHueTransport::new();
        let mut state = active_state(&spy, &temp.storage);
        spy.create_room(
            "user",
            &HueRoomDefinition {
                name: "Nook".to_string(),
                archetype: DEFAULT_MANAGED_HUE_ROOM_ARCHETYPE.to_string(),
                device_ids: vec!["bulb".to_string()],
            },
        )
        .unwrap();
        spy.reset();

        reconcile_managed_rooms(
            &temp.storage,
            &key(),
            &mut state,
            &spy,
            "user",
            &[DesiredHueRoom::new(
                "rhythm-a",
                "Nook",
                vec!["bulb".to_string()],
            )],
            &BTreeSet::from(["bulb".to_string()]),
        )
        .unwrap();

        assert_eq!(state.managed_rooms().len(), 1);
        assert!(!spy
            .calls()
            .iter()
            .any(|call| matches!(call, HueTransportCall::CreateRoom { .. })));
    }

    #[test]
    fn missing_mapping_is_forgotten_durably_before_replacement_create() {
        let temp = TempStorage::new("forget-before-recreate");
        let spy = SpyHueTransport::new();
        let mut state = active_state(&spy, &temp.storage);
        let desired = vec![DesiredHueRoom::new(
            "rhythm-a",
            "Nook",
            vec!["bulb".to_string()],
        )];
        let controlled = BTreeSet::from(["bulb".to_string()]);
        reconcile_managed_rooms(
            &temp.storage,
            &key(),
            &mut state,
            &spy,
            "user",
            &desired,
            &controlled,
        )
        .unwrap();
        let missing_room_id = state.managed_rooms()["rhythm-a"].hue_room_id.clone();
        spy.delete_room("user", &missing_room_id).unwrap();

        let observed = observe_hue_rooms(&spy, "user").unwrap();
        let operation = next_managed_room_operation(&state, &desired, &controlled, &observed)
            .unwrap()
            .unwrap();
        assert_eq!(
            operation,
            HueManagedRoomOperation::ForgetMissing {
                rhythm_room_id: "rhythm-a".to_string(),
            }
        );
        execute_managed_room_operation(&temp.storage, &key(), &mut state, &spy, "user", operation)
            .unwrap();
        let mut reloaded = load_controller_ownership(&temp.storage, "bridge-rooms")
            .unwrap()
            .unwrap();
        assert!(!reloaded.managed_rooms().contains_key("rhythm-a"));

        // Simulate a process dying after Hue accepted the replacement create
        // but before Rhythm could persist its new mapping.
        let orphan = spy
            .create_room(
                "user",
                &HueRoomDefinition {
                    name: "Nook".to_string(),
                    archetype: DEFAULT_MANAGED_HUE_ROOM_ARCHETYPE.to_string(),
                    device_ids: vec!["bulb".to_string()],
                },
            )
            .unwrap();
        spy.reset();

        reconcile_managed_rooms(
            &temp.storage,
            &key(),
            &mut reloaded,
            &spy,
            "user",
            &desired,
            &controlled,
        )
        .unwrap();
        assert_eq!(
            reloaded.managed_rooms()["rhythm-a"].hue_room_id,
            orphan.room_id
        );
        assert!(!spy
            .calls()
            .iter()
            .any(|call| matches!(call, HueTransportCall::CreateRoom { .. })));
    }
}
