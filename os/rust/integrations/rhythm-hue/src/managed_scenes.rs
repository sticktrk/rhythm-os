//! Project Rhythm-owned scenes into hidden Hue room scenes.
//!
//! A scene is addressable only through an explicit managed-room mapping in the
//! bridge ownership manifest. Display names are deliberately generic and are
//! never used as ownership proof.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use rhythm_os::discovery::ManagedSceneProjection;
use rhythm_os::scenes::{LightSceneColor, LightScenePower};
use rhythm_os::storage::Storage;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::ownership::{
    persist_controller_ownership, HueControllerOwnership, HueManagedScene, HueOwnershipPhase,
};
use crate::transport::HueTransport;

const HUE_VENDOR_MIN_MIREK: u32 = 50;
const HUE_VENDOR_MAX_MIREK: u32 = 1000;

/// Bridge inventories reused across the managed projections of one batch.
///
/// A whole-home apply projects many rooms back to back on the same bridge, and
/// every projection needs the current scene inventory plus the device→light
/// service map. Fetching those once per batch instead of once per room keeps
/// the bridge round trips per room down to the mutation (when the scene
/// changed) and the recall. Every mutation is followed by the read-back that
/// verification already requires, and that read-back refreshes the cache, so
/// it cannot go stale within a batch.
#[derive(Debug, Default)]
pub struct ManagedSceneBridgeCache {
    scenes: Option<Vec<Value>>,
    light_services: Option<BTreeMap<String, Vec<String>>>,
}

/// Bridge reads for one managed-scene operation, optionally memoized in a
/// batch cache.
struct BridgeReads<'a, H: HueTransport + ?Sized> {
    transport: &'a H,
    username: &'a str,
    cache: Option<&'a mut ManagedSceneBridgeCache>,
}

impl<'a, H: HueTransport + ?Sized> BridgeReads<'a, H> {
    fn new(
        transport: &'a H,
        username: &'a str,
        cache: Option<&'a mut ManagedSceneBridgeCache>,
    ) -> Self {
        Self {
            transport,
            username,
            cache,
        }
    }

    /// The scene inventory, from the batch cache when one holds it.
    fn scenes(&mut self) -> Result<Vec<Value>> {
        if let Some(scenes) = self
            .cache
            .as_deref()
            .and_then(|cache| cache.scenes.as_ref())
        {
            return Ok(scenes.clone());
        }
        self.refresh_scenes()
    }

    /// Read the scene inventory from the bridge and remember it. Every
    /// mutation is followed by this read so the cache reflects the bridge.
    fn refresh_scenes(&mut self) -> Result<Vec<Value>> {
        let scenes = data_array(
            "scene",
            &self.transport.get_resources(self.username, "scene")?,
        )?
        .to_vec();
        if let Some(cache) = self.cache.as_deref_mut() {
            cache.scenes = Some(scenes.clone());
        }
        Ok(scenes)
    }

    /// Native device ID → light service IDs, from the batch cache when one
    /// holds it. Device services do not change while scenes are projected.
    fn light_services(&mut self) -> Result<BTreeMap<String, Vec<String>>> {
        if let Some(services) = self
            .cache
            .as_deref()
            .and_then(|cache| cache.light_services.as_ref())
        {
            return Ok(services.clone());
        }
        let services = light_services_by_device(self.transport, self.username)?;
        if let Some(cache) = self.cache.as_deref_mut() {
            cache.light_services = Some(services.clone());
        }
        Ok(services)
    }
}

fn data_array<'a>(resource_type: &str, value: &'a Value) -> Result<&'a [Value]> {
    value
        .get("data")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| anyhow::anyhow!("Hue {resource_type} response has no data array"))
}

fn light_services_by_device<H: HueTransport + ?Sized>(
    transport: &H,
    username: &str,
) -> Result<BTreeMap<String, Vec<String>>> {
    let payload = transport.get_resources(username, "device")?;
    let mut services = BTreeMap::new();
    for device in data_array("device", &payload)? {
        let Some(device_id) = device.get("id").and_then(Value::as_str) else {
            continue;
        };
        let mut light_ids = device
            .get("services")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|service| service.get("rtype").and_then(Value::as_str) == Some("light"))
            .filter_map(|service| service.get("rid").and_then(Value::as_str))
            .map(str::to_string)
            .collect::<Vec<_>>();
        light_ids.sort();
        light_ids.dedup();
        if !light_ids.is_empty() {
            services.insert(device_id.to_string(), light_ids);
        }
    }
    Ok(services)
}

fn action_for_output(output: &rhythm_os::scenes::LightSceneOutput) -> Result<Value> {
    if output.power == LightScenePower::Off {
        return Ok(json!({"on": {"on": false}}));
    }

    let mut action = json!({
        "on": {"on": true},
        "dimming": {"brightness": output.brightness.clamp(1, 100) as f64}
    });
    match output.color {
        Some(LightSceneColor::Kelvin { kelvin }) => {
            let mirek = ((1_000_000.0_f32 / kelvin.max(1) as f32).round() as u32)
                .clamp(HUE_VENDOR_MIN_MIREK, HUE_VENDOR_MAX_MIREK);
            action["color_temperature"] = json!({"mirek": mirek});
        }
        Some(LightSceneColor::Rgb { rgb }) => {
            let xy = rhythm_core::rgb_to_xy(rgb);
            action["color"] = json!({"xy": {"x": xy.x, "y": xy.y}});
        }
        Some(LightSceneColor::Xy { xy }) | Some(LightSceneColor::RgbXy { xy, .. }) => {
            if !xy.x.is_finite() || !xy.y.is_finite() {
                anyhow::bail!("Rhythm scene contains an invalid XY color");
            }
            action["color"] = json!({
                "xy": {
                    "x": xy.x.clamp(0.0, 1.0),
                    "y": xy.y.clamp(0.0, 1.0)
                }
            });
        }
        None => anyhow::bail!("A powered-on Rhythm scene output has no color"),
    }
    Ok(action)
}

fn fingerprint(value: &Value) -> Result<String> {
    let encoded = serde_json::to_vec(value)?;
    let digest = Sha256::digest(encoded);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn desired_scene_body<H: HueTransport + ?Sized>(
    reads: &mut BridgeReads<'_, H>,
    projection: &ManagedSceneProjection,
) -> Result<(Value, String)> {
    if projection.targets.is_empty() {
        anyhow::bail!("A managed Hue scene must contain at least one light action");
    }
    let services = reads.light_services()?;
    let mut native_ids = BTreeSet::new();
    let mut actions_by_light = BTreeMap::new();
    for target in &projection.targets {
        if !native_ids.insert(target.native_device_id.as_str()) {
            anyhow::bail!("A managed Hue scene contains a duplicate device action");
        }
        let light_ids = services
            .get(&target.native_device_id)
            .ok_or_else(|| anyhow::anyhow!("A managed Hue scene device has no light service"))?;
        let action = action_for_output(&target.output)?;
        for light_id in light_ids {
            if actions_by_light.insert(light_id, action.clone()).is_some() {
                anyhow::bail!("A Hue light service belongs to multiple projected devices");
            }
        }
    }
    let actions = actions_by_light
        .into_iter()
        .map(|(light_id, action)| {
            json!({
                "target": {"rid": light_id, "rtype": "light"},
                "action": action
            })
        })
        .collect::<Vec<_>>();
    let content = json!({
        "group": {"rid": projection.hub_room_id, "rtype": "room"},
        "actions": actions
    });
    let fingerprint = fingerprint(&content)?;
    let body = json!({
        "metadata": {"name": format!("Rhythm {}", &fingerprint[..16])},
        "group": content["group"].clone(),
        "actions": content["actions"].clone()
    });
    Ok((body, fingerprint))
}

fn normalized_scene_projection(value: &Value) -> Value {
    let mut actions = value
        .get("actions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    actions.sort_by_cached_key(|action| {
        action
            .pointer("/target/rid")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    });
    json!({
        "metadata": {
            "name": value.pointer("/metadata/name").and_then(Value::as_str)
        },
        "group": value.get("group").cloned().unwrap_or(Value::Null),
        "actions": actions
    })
}

fn scene_matches(desired: &Value, observed: &Value) -> bool {
    normalized_scene_projection(desired) == normalized_scene_projection(observed)
}

fn verified_scene<'a>(scenes: &'a [Value], scene_id: &str, desired: &Value) -> Result<&'a Value> {
    let scene = scenes
        .iter()
        .find(|scene| scene.get("id").and_then(Value::as_str) == Some(scene_id))
        .ok_or_else(|| anyhow::anyhow!("A managed Hue scene is missing after a confirmed write"))?;
    if !scene_matches(desired, scene) {
        anyhow::bail!("A managed Hue scene failed semantic verification");
    }
    Ok(scene)
}

fn delete_mapping<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    state: &mut HueControllerOwnership,
    reads: &mut BridgeReads<'_, H>,
    mapping: &HueManagedScene,
) -> Result<()> {
    let scenes = reads.scenes()?;
    if scenes
        .iter()
        .any(|scene| scene.get("id").and_then(Value::as_str) == Some(&mapping.hue_scene_id))
    {
        reads
            .transport
            .delete_resource(reads.username, "scene", &mapping.hue_scene_id)?;
        if reads
            .refresh_scenes()?
            .iter()
            .any(|scene| scene.get("id").and_then(Value::as_str) == Some(&mapping.hue_scene_id))
        {
            anyhow::bail!("A managed Hue scene remains after deletion");
        }
    }
    state.remove_managed_scene(&mapping.rhythm_room_id, &mapping.rhythm_scene_id)?;
    persist_controller_ownership(storage, state)
}

fn cleanup_ephemeral_mappings<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    state: &mut HueControllerOwnership,
    reads: &mut BridgeReads<'_, H>,
) -> Result<()> {
    let mappings = state
        .managed_scenes()
        .values()
        .filter(|mapping| mapping.ephemeral)
        .cloned()
        .collect::<Vec<_>>();
    for mapping in mappings {
        delete_mapping(storage, state, reads, &mapping)?;
    }
    Ok(())
}

fn apply_ephemeral_scene<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    state: &mut HueControllerOwnership,
    reads: &mut BridgeReads<'_, H>,
    projection: &ManagedSceneProjection,
    body: &Value,
    fingerprint: &str,
    transition_ms: Option<u32>,
) -> Result<()> {
    let created = reads
        .transport
        .create_resource(reads.username, "scene", body)?;
    if created.resource_type != "scene" {
        anyhow::bail!("Hue returned the wrong resource type for a scene create");
    }
    verified_scene(&reads.refresh_scenes()?, &created.resource_id, body)?;
    let mapping = HueManagedScene {
        rhythm_room_id: projection.rhythm_room_id.clone(),
        rhythm_scene_id: format!("__preview__{fingerprint}"),
        hue_room_id: projection.hub_room_id.clone(),
        hue_scene_id: created.resource_id,
        fingerprint: fingerprint.to_string(),
        ephemeral: true,
    };
    state.record_managed_scene(mapping.clone())?;
    persist_controller_ownership(storage, state)?;

    let recall_result =
        reads
            .transport
            .recall_scene(reads.username, &mapping.hue_scene_id, transition_ms);
    let cleanup_result = delete_mapping(storage, state, reads, &mapping);
    match (recall_result, cleanup_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(_), Ok(())) => anyhow::bail!("Managed Hue scene preview recall failed"),
        (Ok(()), Err(_)) => anyhow::bail!("Managed Hue scene preview cleanup failed"),
        (Err(_), Err(_)) => {
            anyhow::bail!("Managed Hue scene preview recall and cleanup both failed")
        }
    }
}

/// Create/update/adopt, verify, and recall one Rhythm scene projection.
///
/// `cache` memoizes the bridge's scene and device inventories across the
/// projections of one batch (a whole-home apply); pass `None` for a
/// standalone projection.
#[allow(clippy::too_many_arguments)]
pub fn apply_managed_scene<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    state: &mut HueControllerOwnership,
    transport: &H,
    username: &str,
    projection: &ManagedSceneProjection,
    transition_ms: Option<u32>,
    ephemeral: bool,
    cache: Option<&mut ManagedSceneBridgeCache>,
) -> Result<()> {
    if state.phase != HueOwnershipPhase::Active {
        anyhow::bail!("Hue controller authority is not active");
    }
    let mut reads = BridgeReads::new(transport, username, cache);
    cleanup_ephemeral_mappings(storage, state, &mut reads)?;
    let room = state
        .managed_rooms()
        .get(&projection.rhythm_room_id)
        .ok_or_else(|| anyhow::anyhow!("Rhythm scene has no managed Hue room"))?;
    if room.hue_room_id != projection.hub_room_id {
        anyhow::bail!("Rhythm scene references a stale managed Hue room");
    }
    if projection.targets.is_empty() {
        if ephemeral {
            return Ok(());
        }
        if let Some(mapping) = state
            .managed_scene(&projection.rhythm_room_id, &projection.scene_id)
            .cloned()
        {
            delete_mapping(storage, state, &mut reads, &mapping)?;
        }
        return Ok(());
    }
    let (body, fingerprint) = desired_scene_body(&mut reads, projection)?;
    if ephemeral {
        return apply_ephemeral_scene(
            storage,
            state,
            &mut reads,
            projection,
            &body,
            &fingerprint,
            transition_ms,
        );
    }

    let mut mapping = state
        .managed_scene(&projection.rhythm_room_id, &projection.scene_id)
        .cloned();
    let scenes = reads.scenes()?;
    if let Some(existing) = mapping.as_mut() {
        if existing.hue_room_id != projection.hub_room_id || existing.ephemeral {
            anyhow::bail!("Rhythm scene ownership mapping is inconsistent");
        }
        match scenes
            .iter()
            .find(|scene| scene.get("id").and_then(Value::as_str) == Some(&existing.hue_scene_id))
        {
            Some(observed) if scene_matches(&body, observed) => {}
            Some(_) => {
                reads.transport.update_resource(
                    reads.username,
                    "scene",
                    &existing.hue_scene_id,
                    &body,
                )?;
                verified_scene(&reads.refresh_scenes()?, &existing.hue_scene_id, &body)?;
            }
            None => mapping = None,
        }
    }

    if mapping.is_none() {
        let mut equivalent = scenes
            .iter()
            .filter(|scene| scene_matches(&body, scene))
            .filter_map(|scene| scene.get("id").and_then(Value::as_str))
            .map(str::to_string)
            .collect::<Vec<_>>();
        equivalent.sort();
        equivalent.dedup();
        let hue_scene_id = match equivalent.as_slice() {
            [] => {
                let created = reads
                    .transport
                    .create_resource(reads.username, "scene", &body)?;
                if created.resource_type != "scene" {
                    anyhow::bail!("Hue returned the wrong resource type for a scene create");
                }
                verified_scene(&reads.refresh_scenes()?, &created.resource_id, &body)?;
                created.resource_id
            }
            [scene_id] => scene_id.clone(),
            _ => anyhow::bail!("Multiple equivalent unmanaged Hue scenes prevent safe adoption"),
        };
        mapping = Some(HueManagedScene {
            rhythm_room_id: projection.rhythm_room_id.clone(),
            rhythm_scene_id: projection.scene_id.clone(),
            hue_room_id: projection.hub_room_id.clone(),
            hue_scene_id,
            fingerprint: fingerprint.clone(),
            ephemeral: false,
        });
    }

    let mut mapping = mapping.expect("managed scene mapping was created");
    mapping.fingerprint = fingerprint;
    state.record_managed_scene(mapping.clone())?;
    // Persist native ownership before recall so an interruption never leaves
    // an untracked Rhythm-created scene on the bridge.
    persist_controller_ownership(storage, state)?;
    transport.recall_scene(username, &mapping.hue_scene_id, transition_ms)
}

/// Delete all projected copies of a Rhythm scene across managed Hue rooms.
pub fn delete_managed_scene<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    state: &mut HueControllerOwnership,
    transport: &H,
    username: &str,
    rhythm_scene_id: &str,
) -> Result<()> {
    if state.phase != HueOwnershipPhase::Active {
        anyhow::bail!("Hue controller authority is not active");
    }
    let mut reads = BridgeReads::new(transport, username, None);
    cleanup_ephemeral_mappings(storage, state, &mut reads)?;
    let mappings = state
        .managed_scenes()
        .values()
        .filter(|mapping| !mapping.ephemeral && mapping.rhythm_scene_id == rhythm_scene_id)
        .cloned()
        .collect::<Vec<_>>();
    for mapping in mappings {
        delete_mapping(storage, state, &mut reads, &mapping)?;
    }
    Ok(())
}

/// Retire every managed scene tied to a room before membership changes or the
/// room resource is deleted. This prevents stale actions from targeting bulbs
/// after a seamless Rhythm room move.
pub fn delete_managed_scenes_for_room<H: HueTransport + ?Sized>(
    storage: &dyn Storage,
    state: &mut HueControllerOwnership,
    transport: &H,
    username: &str,
    rhythm_room_id: &str,
) -> Result<()> {
    if state.phase != HueOwnershipPhase::Active {
        anyhow::bail!("Hue controller authority is not active");
    }
    let mut reads = BridgeReads::new(transport, username, None);
    cleanup_ephemeral_mappings(storage, state, &mut reads)?;
    let mappings = state
        .managed_scenes()
        .values()
        .filter(|mapping| mapping.rhythm_room_id == rhythm_room_id)
        .cloned()
        .collect::<Vec<_>>();
    for mapping in mappings {
        delete_mapping(storage, state, &mut reads, &mapping)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ownership::{acquire_authoritative_control, HueManagedRoom};
    use crate::test_support::{HueTransportCall, SpyHueTransport};
    use rhythm_core::{runtime::hub_registry::DeviceType, Rgb};
    use rhythm_os::canonical::identity::HubKey;
    use rhythm_os::discovery::ManagedSceneProjectionTarget;
    use rhythm_os::hub::HubType;
    use rhythm_os::scenes::{LightSceneOutput, LightScenePower};
    use rhythm_os::storage::FileStorage;

    struct TempStorage {
        path: std::path::PathBuf,
        storage: FileStorage,
    }

    impl TempStorage {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "rhythm-hue-managed-scenes-{name}-{}",
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
        HubKey::new(HubType::new(HubType::HUE), "192.0.2.50")
    }

    fn active_state(spy: &SpyHueTransport, storage: &FileStorage) -> HueControllerOwnership {
        for (resource_type, data) in [
            ("bridge", json!([{"id": "bridge-scenes"}])),
            (
                "device",
                json!([{
                    "id": "bulb",
                    "services": [{"rid": "light-service", "rtype": "light"}]
                }]),
            ),
            ("light", json!([{"id": "light-service"}])),
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
        let mut state = acquire_authoritative_control(storage, &key(), spy, "user").unwrap();
        state
            .record_managed_room(HueManagedRoom {
                rhythm_room_id: "room".to_string(),
                hue_room_id: "hue-room".to_string(),
                grouped_light_id: "grouped".to_string(),
            })
            .unwrap();
        persist_controller_ownership(storage, &state).unwrap();
        state
    }

    fn projection(brightness: u8) -> ManagedSceneProjection {
        let _ = DeviceType::Light;
        ManagedSceneProjection {
            rhythm_room_id: "room".to_string(),
            hub_room_id: "hue-room".to_string(),
            scene_id: "sunset".to_string(),
            targets: vec![ManagedSceneProjectionTarget {
                native_device_id: "bulb".to_string(),
                output: LightSceneOutput {
                    power: LightScenePower::On,
                    brightness,
                    color: Some(LightSceneColor::Rgb {
                        rgb: Rgb::new(255, 80, 20),
                    }),
                    transition_ms: None,
                },
            }],
        }
    }

    #[test]
    fn persistent_projection_is_created_recalled_idempotently_updated_and_deleted() {
        let temp = TempStorage::new("lifecycle");
        let spy = SpyHueTransport::new();
        let mut state = active_state(&spy, &temp.storage);
        spy.reset();

        apply_managed_scene(
            &temp.storage,
            &mut state,
            &spy,
            "user",
            &projection(55),
            Some(400),
            false,
            None,
        )
        .unwrap();
        assert_eq!(state.managed_scenes().len(), 1);
        assert_eq!(
            spy.calls()
                .iter()
                .filter(|call| matches!(call, HueTransportCall::CreateResource { resource_type, .. } if resource_type == "scene"))
                .count(),
            1
        );

        spy.reset();
        apply_managed_scene(
            &temp.storage,
            &mut state,
            &spy,
            "user",
            &projection(55),
            None,
            false,
            None,
        )
        .unwrap();
        assert!(!spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::CreateResource { resource_type, .. }
                | HueTransportCall::UpdateResource { resource_type, .. }
                if resource_type == "scene"
        )));

        spy.reset();
        apply_managed_scene(
            &temp.storage,
            &mut state,
            &spy,
            "user",
            &projection(70),
            None,
            false,
            None,
        )
        .unwrap();
        assert!(spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::UpdateResource { resource_type, .. } if resource_type == "scene"
        )));

        spy.reset();
        delete_managed_scene(&temp.storage, &mut state, &spy, "user", "sunset").unwrap();
        assert!(state.managed_scenes().is_empty());
        assert!(spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::DeleteResource { resource_type, .. } if resource_type == "scene"
        )));
    }

    #[test]
    fn draft_preview_is_recalled_then_removed_without_a_final_mapping() {
        let temp = TempStorage::new("preview");
        let spy = SpyHueTransport::new();
        let mut state = active_state(&spy, &temp.storage);
        spy.reset();

        apply_managed_scene(
            &temp.storage,
            &mut state,
            &spy,
            "user",
            &projection(45),
            Some(250),
            true,
            None,
        )
        .unwrap();

        assert!(state.managed_scenes().is_empty());
        let calls = spy.calls();
        let recall = calls
            .iter()
            .position(|call| matches!(call, HueTransportCall::RecallScene { .. }))
            .unwrap();
        let delete = calls
            .iter()
            .position(|call| matches!(call, HueTransportCall::DeleteResource { resource_type, .. } if resource_type == "scene"))
            .unwrap();
        assert!(recall < delete);
    }

    #[test]
    fn a_batch_cache_reads_the_bridge_inventories_once_across_projections() {
        let temp = TempStorage::new("batch-cache");
        let spy = SpyHueTransport::new();
        let mut state = active_state(&spy, &temp.storage);
        let mut cache = ManagedSceneBridgeCache::default();
        let inventory_reads = |spy: &SpyHueTransport| {
            spy.calls()
                .iter()
                .filter(|call| {
                    matches!(
                        call,
                        HueTransportCall::GetResources { resource_type }
                            if resource_type == "scene" || resource_type == "device"
                    )
                })
                .count()
        };

        // First projection in the batch: one device read, one scene read,
        // then the create and its verifying read-back.
        spy.reset();
        apply_managed_scene(
            &temp.storage,
            &mut state,
            &spy,
            "user",
            &projection(55),
            Some(400),
            false,
            Some(&mut cache),
        )
        .unwrap();
        assert_eq!(inventory_reads(&spy), 3);

        // An unchanged projection later in the same batch touches the bridge
        // only to recall: every inventory comes from the cache.
        spy.reset();
        apply_managed_scene(
            &temp.storage,
            &mut state,
            &spy,
            "user",
            &projection(55),
            None,
            false,
            Some(&mut cache),
        )
        .unwrap();
        assert_eq!(inventory_reads(&spy), 0);
        assert_eq!(
            spy.calls()
                .iter()
                .filter(|call| matches!(call, HueTransportCall::RecallScene { .. }))
                .count(),
            1
        );

        // A changed projection updates the scene; the verifying read-back is
        // the only inventory read and it refreshes the cache.
        spy.reset();
        apply_managed_scene(
            &temp.storage,
            &mut state,
            &spy,
            "user",
            &projection(70),
            None,
            false,
            Some(&mut cache),
        )
        .unwrap();
        assert_eq!(inventory_reads(&spy), 1);
        assert!(spy.calls().iter().any(|call| matches!(
            call,
            HueTransportCall::UpdateResource { resource_type, .. } if resource_type == "scene"
        )));

        spy.reset();
        apply_managed_scene(
            &temp.storage,
            &mut state,
            &spy,
            "user",
            &projection(70),
            None,
            false,
            Some(&mut cache),
        )
        .unwrap();
        assert_eq!(inventory_reads(&spy), 0, "the refreshed cache is reused");
    }
}
