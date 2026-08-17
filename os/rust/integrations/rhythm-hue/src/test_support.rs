//! Test support utilities for rhythm-hue.
//!
//! Provides `SpyHueTransport` — a recording implementation of `HueTransport`
//! for use in integration tests. Accessible cross-crate when `test-support`
//! is enabled.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::transport::{HueCreatedResource, HueCreatedRoom, HueRoomDefinition, HueTransport};

/// `HueTransport` impl for `Arc<SpyHueTransport>` — allows sharing the spy
/// between the controller (which takes ownership of the transport) and the test
/// (which inspects recorded calls).
impl HueTransport for Arc<SpyHueTransport> {
    fn test_connection(&self, username: &str) -> anyhow::Result<bool> {
        (**self).test_connection(username)
    }
    fn warmup_tls(&self) -> anyhow::Result<()> {
        (**self).warmup_tls()
    }
    fn set_grouped_light(
        &self,
        username: &str,
        grouped_light_id: &str,
        on: bool,
        brightness: Option<u8>,
        kelvin: Option<u16>,
        xy: Option<(f32, f32)>,
        fade_ms: Option<u16>,
    ) -> anyhow::Result<()> {
        (**self).set_grouped_light(
            username,
            grouped_light_id,
            on,
            brightness,
            kelvin,
            xy,
            fade_ms,
        )
    }
    fn set_light(
        &self,
        username: &str,
        light_id: &str,
        on: bool,
        brightness: Option<u8>,
        kelvin: Option<u16>,
        xy: Option<(f32, f32)>,
        fade_ms: Option<u16>,
    ) -> anyhow::Result<()> {
        (**self).set_light(username, light_id, on, brightness, kelvin, xy, fade_ms)
    }
    fn is_grouped_light_on(&self, username: &str, grouped_light_id: &str) -> anyhow::Result<bool> {
        (**self).is_grouped_light_on(username, grouped_light_id)
    }
    fn is_light_on(&self, username: &str, light_id: &str) -> anyhow::Result<bool> {
        (**self).is_light_on(username, light_id)
    }
    fn identify_light(&self, username: &str, light_id: &str) -> anyhow::Result<()> {
        (**self).identify_light(username, light_id)
    }
    fn get_resources(
        &self,
        username: &str,
        resource_type: &str,
    ) -> anyhow::Result<serde_json::Value> {
        (**self).get_resources(username, resource_type)
    }
    fn get_all_resources(&self, username: &str) -> anyhow::Result<serde_json::Value> {
        (**self).get_all_resources(username)
    }
    fn create_resource(
        &self,
        username: &str,
        resource_type: &str,
        body: &serde_json::Value,
    ) -> anyhow::Result<HueCreatedResource> {
        (**self).create_resource(username, resource_type, body)
    }
    fn update_resource(
        &self,
        username: &str,
        resource_type: &str,
        resource_id: &str,
        body: &serde_json::Value,
    ) -> anyhow::Result<()> {
        (**self).update_resource(username, resource_type, resource_id, body)
    }
    fn delete_resource(
        &self,
        username: &str,
        resource_type: &str,
        resource_id: &str,
    ) -> anyhow::Result<()> {
        (**self).delete_resource(username, resource_type, resource_id)
    }
    fn get_v1(&self, username: &str, path: &str) -> anyhow::Result<serde_json::Value> {
        (**self).get_v1(username, path)
    }
    fn post_v1(
        &self,
        username: &str,
        path: &str,
        body: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        (**self).post_v1(username, path, body)
    }
    fn put_v1(
        &self,
        username: &str,
        path: &str,
        body: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        (**self).put_v1(username, path, body)
    }
    fn delete_v1(&self, username: &str, path: &str) -> anyhow::Result<serde_json::Value> {
        (**self).delete_v1(username, path)
    }
    fn recall_scene(
        &self,
        username: &str,
        scene_id: &str,
        transition_ms: Option<u32>,
    ) -> anyhow::Result<()> {
        (**self).recall_scene(username, scene_id, transition_ms)
    }
    fn update_room_children(
        &self,
        username: &str,
        room_id: &str,
        device_ids: &[String],
    ) -> anyhow::Result<()> {
        (**self).update_room_children(username, room_id, device_ids)
    }
    fn create_room(
        &self,
        username: &str,
        definition: &HueRoomDefinition,
    ) -> anyhow::Result<HueCreatedRoom> {
        (**self).create_room(username, definition)
    }
    fn update_room(
        &self,
        username: &str,
        room_id: &str,
        definition: &HueRoomDefinition,
    ) -> anyhow::Result<()> {
        (**self).update_room(username, room_id, definition)
    }
    fn rename_room(&self, username: &str, room_id: &str, name: &str) -> anyhow::Result<()> {
        (**self).rename_room(username, room_id, name)
    }
    fn rename_device(&self, username: &str, device_id: &str, name: &str) -> anyhow::Result<()> {
        (**self).rename_device(username, device_id, name)
    }
    fn delete_room(&self, username: &str, room_id: &str) -> anyhow::Result<()> {
        (**self).delete_room(username, room_id)
    }
}

/// A recorded call to a `SpyHueTransport`.
#[derive(Debug, Clone, PartialEq)]
pub enum HueTransportCall {
    SetGroupedLight {
        grouped_light_id: String,
        on: bool,
        brightness: Option<u8>,
        kelvin: Option<u16>,
        xy: Option<(f32, f32)>,
        fade_ms: Option<u16>,
    },
    SetLight {
        light_id: String,
        on: bool,
        brightness: Option<u8>,
        kelvin: Option<u16>,
        xy: Option<(f32, f32)>,
        fade_ms: Option<u16>,
    },
    IsGroupedLightOn {
        grouped_light_id: String,
    },
    IsLightOn {
        light_id: String,
    },
    IdentifyLight {
        light_id: String,
    },
    GetResources {
        resource_type: String,
    },
    CreateResource {
        resource_type: String,
        body: serde_json::Value,
    },
    UpdateResource {
        resource_type: String,
        resource_id: String,
        body: serde_json::Value,
    },
    DeleteResource {
        resource_type: String,
        resource_id: String,
    },
    GetV1 {
        path: String,
    },
    PostV1 {
        path: String,
        body: serde_json::Value,
    },
    PutV1 {
        path: String,
        body: serde_json::Value,
    },
    DeleteV1 {
        path: String,
    },
    RecallScene {
        scene_id: String,
        transition_ms: Option<u32>,
    },
    UpdateRoomChildren {
        room_id: String,
        device_ids: Vec<String>,
    },
    CreateRoom {
        definition: HueRoomDefinition,
    },
    UpdateRoom {
        room_id: String,
        definition: HueRoomDefinition,
    },
    RenameRoom {
        room_id: String,
        name: String,
    },
    RenameDevice {
        device_id: String,
        name: String,
    },
    DeleteRoom {
        room_id: String,
    },
    TestConnection,
    WarmupTls,
}

/// A recording `HueTransport` implementation for testing.
///
/// Records all transport calls and returns configurable responses.
/// Thread-safe — all interior state is behind `Arc<Mutex<_>>` or atomics.
pub struct SpyHueTransport {
    calls: Arc<Mutex<Vec<HueTransportCall>>>,
    is_on: Arc<Mutex<bool>>,
    resources: Arc<Mutex<HashMap<String, serde_json::Value>>>,
    should_fail: Arc<AtomicBool>,
    ignore_resource_mutations: Arc<AtomicBool>,
    fail_next_resource_read_after_update: Arc<AtomicBool>,
    resource_read_failure_armed: Arc<AtomicBool>,
    fail_resource_update: Arc<Mutex<Option<(String, String)>>>,
    fail_room_update: Arc<Mutex<Option<String>>>,
    v1_write_response_override: Arc<Mutex<Option<serde_json::Value>>>,
    next_resource_id: Arc<AtomicUsize>,
}

impl SpyHueTransport {
    /// Create a new spy transport and return shared handles for inspection.
    pub fn new() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            is_on: Arc::new(Mutex::new(false)),
            resources: Arc::new(Mutex::new(HashMap::new())),
            should_fail: Arc::new(AtomicBool::new(false)),
            ignore_resource_mutations: Arc::new(AtomicBool::new(false)),
            fail_next_resource_read_after_update: Arc::new(AtomicBool::new(false)),
            resource_read_failure_armed: Arc::new(AtomicBool::new(false)),
            fail_resource_update: Arc::new(Mutex::new(None)),
            fail_room_update: Arc::new(Mutex::new(None)),
            v1_write_response_override: Arc::new(Mutex::new(None)),
            next_resource_id: Arc::new(AtomicUsize::new(1)),
        }
    }

    /// Set whether `is_grouped_light_on` returns true or false.
    pub fn set_is_on(&self, on: bool) {
        *self.is_on.lock().unwrap() = on;
    }

    /// Set whether transport calls should return errors.
    pub fn set_should_fail(&self, fail: bool) {
        self.should_fail.store(fail, Ordering::Relaxed);
    }

    /// Acknowledge resource mutations without changing subsequent reads.
    /// This simulates application-level success whose effect is not durable.
    pub fn set_ignore_resource_mutations(&self, ignore: bool) {
        self.ignore_resource_mutations
            .store(ignore, Ordering::Relaxed);
    }

    /// Apply the next resource update, then fail its first typed read-back.
    /// This models an acknowledged write followed by a transient observation
    /// failure without making later recovery reads fail.
    pub fn set_fail_next_resource_read_after_update(&self, fail: bool) {
        self.fail_next_resource_read_after_update
            .store(fail, Ordering::Relaxed);
    }

    /// Configure one generic V2 resource update that should fail.
    pub fn set_fail_resource_update(&self, resource_type: &str, resource_id: &str) {
        *self.fail_resource_update.lock().unwrap() =
            Some((resource_type.to_string(), resource_id.to_string()));
    }

    /// Configure one room ID whose membership update should fail.
    pub fn set_fail_room_update(&self, room_id: Option<&str>) {
        *self.fail_room_update.lock().unwrap() = room_id.map(str::to_string);
    }

    /// Override the next and subsequent generic V1 write receipts.
    pub fn set_v1_write_response_override(&self, response: Option<serde_json::Value>) {
        *self.v1_write_response_override.lock().unwrap() = response;
    }

    /// Configure the response returned by `get_resources` for a resource path.
    pub fn set_resource_response(&self, resource_type: &str, response: serde_json::Value) {
        self.resources
            .lock()
            .unwrap()
            .insert(resource_type.to_string(), response);
    }

    /// Configure a Hue V1 response under a path relative to `/api/{username}`.
    pub fn set_v1_response(&self, path: &str, response: serde_json::Value) {
        self.set_resource_response(&format!("v1:{}", path.trim_matches('/')), response);
    }

    /// Get all recorded transport calls.
    pub fn calls(&self) -> Vec<HueTransportCall> {
        self.calls.lock().unwrap().clone()
    }

    /// Get only `SetGroupedLight` calls.
    pub fn set_grouped_light_calls(&self) -> Vec<HueTransportCall> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| matches!(c, HueTransportCall::SetGroupedLight { .. }))
            .cloned()
            .collect()
    }

    /// Get only `SetLight` calls.
    pub fn set_light_calls(&self) -> Vec<HueTransportCall> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| matches!(c, HueTransportCall::SetLight { .. }))
            .cloned()
            .collect()
    }

    /// Count `SetGroupedLight` calls.
    pub fn set_grouped_light_count(&self) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| matches!(c, HueTransportCall::SetGroupedLight { .. }))
            .count()
    }

    /// Clear all recorded calls.
    pub fn reset(&self) {
        self.calls.lock().unwrap().clear();
    }
}

impl Default for SpyHueTransport {
    fn default() -> Self {
        Self::new()
    }
}

fn merge_json(target: &mut serde_json::Value, update: &serde_json::Value) {
    match (target, update) {
        (serde_json::Value::Object(target), serde_json::Value::Object(update)) => {
            for (key, value) in update {
                merge_json(
                    target.entry(key.clone()).or_insert(serde_json::Value::Null),
                    value,
                );
            }
        }
        (target, update) => *target = update.clone(),
    }
}

fn resource_data_mut<'a>(
    resources: &'a mut HashMap<String, serde_json::Value>,
    resource_type: &str,
) -> anyhow::Result<&'a mut Vec<serde_json::Value>> {
    resources
        .entry(resource_type.to_string())
        .or_insert_with(|| serde_json::json!({"data": [], "errors": []}))
        .get_mut("data")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| anyhow::anyhow!("spy: resource response has no data array: {resource_type}"))
}

fn room_json(room_id: &str, definition: &HueRoomDefinition) -> serde_json::Value {
    serde_json::json!({
        "id": room_id,
        "children": definition.device_ids.iter().map(|device_id| {
            serde_json::json!({"rid": device_id, "rtype": "device"})
        }).collect::<Vec<_>>(),
        "metadata": {
            "name": definition.name,
            "archetype": definition.archetype,
        },
        "services": [
            {"rid": format!("grouped-{room_id}"), "rtype": "grouped_light"}
        ]
    })
}

impl HueTransport for SpyHueTransport {
    fn test_connection(&self, _username: &str) -> anyhow::Result<bool> {
        self.calls
            .lock()
            .unwrap()
            .push(HueTransportCall::TestConnection);
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: connection failed");
        }
        Ok(true)
    }

    fn warmup_tls(&self) -> anyhow::Result<()> {
        self.calls.lock().unwrap().push(HueTransportCall::WarmupTls);
        Ok(())
    }

    fn set_grouped_light(
        &self,
        _username: &str,
        grouped_light_id: &str,
        on: bool,
        brightness: Option<u8>,
        kelvin: Option<u16>,
        xy: Option<(f32, f32)>,
        fade_ms: Option<u16>,
    ) -> anyhow::Result<()> {
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: set_grouped_light failed");
        }
        self.calls
            .lock()
            .unwrap()
            .push(HueTransportCall::SetGroupedLight {
                grouped_light_id: grouped_light_id.to_string(),
                on,
                brightness,
                kelvin,
                xy,
                fade_ms,
            });
        Ok(())
    }

    fn set_light(
        &self,
        _username: &str,
        light_id: &str,
        on: bool,
        brightness: Option<u8>,
        kelvin: Option<u16>,
        xy: Option<(f32, f32)>,
        fade_ms: Option<u16>,
    ) -> anyhow::Result<()> {
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: set_light failed");
        }
        self.calls.lock().unwrap().push(HueTransportCall::SetLight {
            light_id: light_id.to_string(),
            on,
            brightness,
            kelvin,
            xy,
            fade_ms,
        });
        Ok(())
    }

    fn is_grouped_light_on(&self, _username: &str, grouped_light_id: &str) -> anyhow::Result<bool> {
        self.calls
            .lock()
            .unwrap()
            .push(HueTransportCall::IsGroupedLightOn {
                grouped_light_id: grouped_light_id.to_string(),
            });
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: is_grouped_light_on failed");
        }
        Ok(*self.is_on.lock().unwrap())
    }

    fn is_light_on(&self, _username: &str, light_id: &str) -> anyhow::Result<bool> {
        self.calls
            .lock()
            .unwrap()
            .push(HueTransportCall::IsLightOn {
                light_id: light_id.to_string(),
            });
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: is_light_on failed");
        }
        Ok(*self.is_on.lock().unwrap())
    }

    fn identify_light(&self, _username: &str, light_id: &str) -> anyhow::Result<()> {
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: identify_light failed");
        }
        self.calls
            .lock()
            .unwrap()
            .push(HueTransportCall::IdentifyLight {
                light_id: light_id.to_string(),
            });
        Ok(())
    }

    fn get_resources(
        &self,
        _username: &str,
        resource_type: &str,
    ) -> anyhow::Result<serde_json::Value> {
        self.calls
            .lock()
            .unwrap()
            .push(HueTransportCall::GetResources {
                resource_type: resource_type.to_string(),
            });
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: get_resources failed");
        }
        if self
            .resource_read_failure_armed
            .swap(false, Ordering::Relaxed)
        {
            anyhow::bail!("spy: post-update get_resources failed");
        }
        Ok(self
            .resources
            .lock()
            .unwrap()
            .get(resource_type)
            .cloned()
            .unwrap_or_else(|| serde_json::json!({"data": []})))
    }

    fn get_all_resources(&self, _username: &str) -> anyhow::Result<serde_json::Value> {
        self.calls
            .lock()
            .unwrap()
            .push(HueTransportCall::GetResources {
                resource_type: "*".to_string(),
            });
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: get_all_resources failed");
        }
        let resources = self.resources.lock().unwrap();
        let mut inventory = Vec::new();
        for (resource_type, payload) in resources.iter() {
            if resource_type.starts_with("v1:") {
                continue;
            }
            for resource in payload
                .get("data")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
            {
                let mut resource = resource.clone();
                if resource.get("type").is_none() {
                    resource["type"] = serde_json::Value::String(resource_type.clone());
                }
                inventory.push(resource);
            }
        }
        inventory.sort_by_cached_key(|resource| {
            (
                resource
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                resource
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        });
        Ok(serde_json::json!({"data": inventory, "errors": []}))
    }

    fn create_resource(
        &self,
        _username: &str,
        resource_type: &str,
        body: &serde_json::Value,
    ) -> anyhow::Result<HueCreatedResource> {
        self.calls
            .lock()
            .unwrap()
            .push(HueTransportCall::CreateResource {
                resource_type: resource_type.to_string(),
                body: body.clone(),
            });
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: create_resource failed");
        }
        let sequence = self.next_resource_id.fetch_add(1, Ordering::Relaxed);
        let resource_id = format!("rhythm-{resource_type}-{sequence}");
        let mut resource = body.clone();
        resource["id"] = serde_json::Value::String(resource_id.clone());
        if resource_type == "room" {
            resource["services"] = serde_json::json!([
                {"rid": format!("grouped-{resource_id}"), "rtype": "grouped_light"}
            ]);
        }
        resource_data_mut(&mut self.resources.lock().unwrap(), resource_type)?.push(resource);
        Ok(HueCreatedResource {
            resource_id,
            resource_type: resource_type.to_string(),
        })
    }

    fn update_resource(
        &self,
        _username: &str,
        resource_type: &str,
        resource_id: &str,
        body: &serde_json::Value,
    ) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(HueTransportCall::UpdateResource {
                resource_type: resource_type.to_string(),
                resource_id: resource_id.to_string(),
                body: body.clone(),
            });
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: update_resource failed");
        }
        if self
            .fail_resource_update
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|(failed_type, failed_id)| {
                failed_type == resource_type && failed_id == resource_id
            })
        {
            anyhow::bail!("spy: targeted update_resource failed");
        }
        if self.ignore_resource_mutations.load(Ordering::Relaxed) {
            return Ok(());
        }
        let mut resources = self.resources.lock().unwrap();
        let resource = resource_data_mut(&mut resources, resource_type)?
            .iter_mut()
            .find(|resource| {
                resource.get("id").and_then(serde_json::Value::as_str) == Some(resource_id)
            })
            .ok_or_else(|| anyhow::anyhow!("spy: resource not found"))?;
        merge_json(resource, body);
        if self
            .fail_next_resource_read_after_update
            .swap(false, Ordering::Relaxed)
        {
            self.resource_read_failure_armed
                .store(true, Ordering::Relaxed);
        }
        Ok(())
    }

    fn delete_resource(
        &self,
        _username: &str,
        resource_type: &str,
        resource_id: &str,
    ) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(HueTransportCall::DeleteResource {
                resource_type: resource_type.to_string(),
                resource_id: resource_id.to_string(),
            });
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: delete_resource failed");
        }
        if self.ignore_resource_mutations.load(Ordering::Relaxed) {
            return Ok(());
        }
        let mut resources = self.resources.lock().unwrap();
        resource_data_mut(&mut resources, resource_type)?.retain(|resource| {
            resource.get("id").and_then(serde_json::Value::as_str) != Some(resource_id)
        });
        Ok(())
    }

    fn get_v1(&self, _username: &str, path: &str) -> anyhow::Result<serde_json::Value> {
        let path = path.trim_matches('/');
        self.calls.lock().unwrap().push(HueTransportCall::GetV1 {
            path: path.to_string(),
        });
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: get_v1 failed");
        }
        Ok(self
            .resources
            .lock()
            .unwrap()
            .get(&format!("v1:{path}"))
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})))
    }

    fn post_v1(
        &self,
        _username: &str,
        path: &str,
        body: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let path = path.trim_matches('/');
        self.calls.lock().unwrap().push(HueTransportCall::PostV1 {
            path: path.to_string(),
            body: body.clone(),
        });
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: post_v1 failed");
        }
        if let Some(response) = self.v1_write_response_override.lock().unwrap().clone() {
            return Ok(response);
        }
        Ok(serde_json::json!([{"success": {format!("/{path}"): "created"}}]))
    }

    fn put_v1(
        &self,
        _username: &str,
        path: &str,
        body: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let path = path.trim_matches('/');
        self.calls.lock().unwrap().push(HueTransportCall::PutV1 {
            path: path.to_string(),
            body: body.clone(),
        });
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: put_v1 failed");
        }
        if let Some((collection, id)) = path.split_once('/') {
            if let Some(resource) = self
                .resources
                .lock()
                .unwrap()
                .get_mut(&format!("v1:{collection}"))
                .and_then(serde_json::Value::as_object_mut)
                .and_then(|resources| resources.get_mut(id))
            {
                merge_json(resource, body);
            }
        }
        if let Some(response) = self.v1_write_response_override.lock().unwrap().clone() {
            return Ok(response);
        }
        let success = body
            .as_object()
            .into_iter()
            .flat_map(|body| body.iter())
            .map(|(field, value)| (format!("/{path}/{field}"), value.clone()))
            .collect::<serde_json::Map<_, _>>();
        Ok(serde_json::json!([{"success": success}]))
    }

    fn delete_v1(&self, _username: &str, path: &str) -> anyhow::Result<serde_json::Value> {
        let path = path.trim_matches('/');
        self.calls.lock().unwrap().push(HueTransportCall::DeleteV1 {
            path: path.to_string(),
        });
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: delete_v1 failed");
        }
        if let Some((collection, id)) = path.split_once('/') {
            self.resources
                .lock()
                .unwrap()
                .get_mut(&format!("v1:{collection}"))
                .and_then(serde_json::Value::as_object_mut)
                .map(|resources| resources.remove(id));
        }
        if let Some(response) = self.v1_write_response_override.lock().unwrap().clone() {
            return Ok(response);
        }
        Ok(serde_json::json!([{"success": format!("/{path} deleted.")}]))
    }

    fn recall_scene(
        &self,
        _username: &str,
        scene_id: &str,
        transition_ms: Option<u32>,
    ) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(HueTransportCall::RecallScene {
                scene_id: scene_id.to_string(),
                transition_ms,
            });
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: recall_scene failed");
        }
        Ok(())
    }

    fn update_room_children(
        &self,
        _username: &str,
        room_id: &str,
        device_ids: &[String],
    ) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(HueTransportCall::UpdateRoomChildren {
                room_id: room_id.to_string(),
                device_ids: device_ids.to_vec(),
            });
        if self.should_fail.load(Ordering::Relaxed)
            || self.fail_room_update.lock().unwrap().as_deref() == Some(room_id)
        {
            anyhow::bail!("spy: update_room_children failed");
        }
        if self.ignore_resource_mutations.load(Ordering::Relaxed) {
            return Ok(());
        }

        let mut resources = self.resources.lock().unwrap();
        let rooms = resources
            .entry("room".to_string())
            .or_insert_with(|| serde_json::json!({"data": []}));
        let room = rooms
            .get_mut("data")
            .and_then(|value| value.as_array_mut())
            .and_then(|rooms| {
                rooms
                    .iter_mut()
                    .find(|room| room.get("id").and_then(|value| value.as_str()) == Some(room_id))
            })
            .ok_or_else(|| anyhow::anyhow!("spy: room not found"))?;
        room["children"] = serde_json::Value::Array(
            device_ids
                .iter()
                .map(|device_id| serde_json::json!({"rid": device_id, "rtype": "device"}))
                .collect(),
        );
        Ok(())
    }

    fn create_room(
        &self,
        _username: &str,
        definition: &HueRoomDefinition,
    ) -> anyhow::Result<HueCreatedRoom> {
        self.calls
            .lock()
            .unwrap()
            .push(HueTransportCall::CreateRoom {
                definition: definition.clone(),
            });
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: create_room failed");
        }
        let sequence = self.next_resource_id.fetch_add(1, Ordering::Relaxed);
        let room_id = format!("rhythm-room-{sequence}");
        if self.ignore_resource_mutations.load(Ordering::Relaxed) {
            return Ok(HueCreatedRoom { room_id });
        }
        resource_data_mut(&mut self.resources.lock().unwrap(), "room")?
            .push(room_json(&room_id, definition));
        Ok(HueCreatedRoom { room_id })
    }

    fn update_room(
        &self,
        _username: &str,
        room_id: &str,
        definition: &HueRoomDefinition,
    ) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(HueTransportCall::UpdateRoom {
                room_id: room_id.to_string(),
                definition: definition.clone(),
            });
        if self.should_fail.load(Ordering::Relaxed)
            || self.fail_room_update.lock().unwrap().as_deref() == Some(room_id)
        {
            anyhow::bail!("spy: update_room failed");
        }
        if self.ignore_resource_mutations.load(Ordering::Relaxed) {
            return Ok(());
        }
        let mut resources = self.resources.lock().unwrap();
        let room = resource_data_mut(&mut resources, "room")?
            .iter_mut()
            .find(|room| room.get("id").and_then(serde_json::Value::as_str) == Some(room_id))
            .ok_or_else(|| anyhow::anyhow!("spy: room not found"))?;
        let services = room.get("services").cloned();
        *room = room_json(room_id, definition);
        if let Some(services) = services {
            room["services"] = services;
        }
        Ok(())
    }

    fn rename_room(&self, _username: &str, room_id: &str, name: &str) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(HueTransportCall::RenameRoom {
                room_id: room_id.to_string(),
                name: name.to_string(),
            });
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: rename_room failed");
        }
        if self.ignore_resource_mutations.load(Ordering::Relaxed) {
            return Ok(());
        }
        let mut resources = self.resources.lock().unwrap();
        let room = resource_data_mut(&mut resources, "room")?
            .iter_mut()
            .find(|room| room.get("id").and_then(serde_json::Value::as_str) == Some(room_id))
            .ok_or_else(|| anyhow::anyhow!("spy: room not found"))?;
        room["metadata"]["name"] = serde_json::Value::String(name.to_string());
        Ok(())
    }

    fn rename_device(&self, _username: &str, device_id: &str, name: &str) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(HueTransportCall::RenameDevice {
                device_id: device_id.to_string(),
                name: name.to_string(),
            });
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: rename_device failed");
        }
        if self.ignore_resource_mutations.load(Ordering::Relaxed) {
            return Ok(());
        }
        let mut resources = self.resources.lock().unwrap();
        let device = resource_data_mut(&mut resources, "device")?
            .iter_mut()
            .find(|device| device.get("id").and_then(serde_json::Value::as_str) == Some(device_id))
            .ok_or_else(|| anyhow::anyhow!("spy: device not found"))?;
        device["metadata"]["name"] = serde_json::Value::String(name.to_string());
        Ok(())
    }

    fn delete_room(&self, _username: &str, room_id: &str) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(HueTransportCall::DeleteRoom {
                room_id: room_id.to_string(),
            });
        if self.should_fail.load(Ordering::Relaxed) {
            anyhow::bail!("spy: delete_room failed");
        }
        if self.ignore_resource_mutations.load(Ordering::Relaxed) {
            return Ok(());
        }
        let mut resources = self.resources.lock().unwrap();
        resource_data_mut(&mut resources, "room")?
            .retain(|room| room.get("id").and_then(serde_json::Value::as_str) != Some(room_id));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_set_grouped_light() {
        let spy = SpyHueTransport::new();
        spy.set_grouped_light("user", "gl1", true, Some(80), Some(4000), None, Some(500))
            .unwrap();

        let calls = spy.set_grouped_light_calls();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            HueTransportCall::SetGroupedLight {
                grouped_light_id,
                on,
                brightness,
                kelvin,
                xy: _,
                fade_ms,
            } => {
                assert_eq!(grouped_light_id, "gl1");
                assert!(*on);
                assert_eq!(*brightness, Some(80));
                assert_eq!(*kelvin, Some(4000));
                assert_eq!(*fade_ms, Some(500));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn should_fail_returns_error() {
        let spy = SpyHueTransport::new();
        spy.set_should_fail(true);
        assert!(spy
            .set_grouped_light("user", "gl1", true, None, None, None, None)
            .is_err());
        assert!(spy
            .set_light("user", "light1", true, None, None, None, None)
            .is_err());
    }

    #[test]
    fn is_on_configurable() {
        let spy = SpyHueTransport::new();
        assert!(!spy.is_grouped_light_on("user", "gl1").unwrap());
        assert!(!spy.is_light_on("user", "light1").unwrap());
        spy.set_is_on(true);
        assert!(spy.is_grouped_light_on("user", "gl1").unwrap());
        assert!(spy.is_light_on("user", "light1").unwrap());
    }

    #[test]
    fn reset_clears_calls() {
        let spy = SpyHueTransport::new();
        spy.set_grouped_light("user", "gl1", true, None, None, None, None)
            .unwrap();
        assert_eq!(spy.set_grouped_light_count(), 1);
        spy.reset();
        assert_eq!(spy.set_grouped_light_count(), 0);
    }
}
