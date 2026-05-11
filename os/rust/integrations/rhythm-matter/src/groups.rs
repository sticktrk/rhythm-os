//! Matter light group management.
//!
//! Matter group addressing differs from Hue/HA room control: the controller
//! must provision a fabric-local group on member endpoints, then commands are
//! sent to a u16 group ID instead of a node/endpoint pair.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use log::{info, warn};
use rhythm_core::groups::{
    group_name_for_area, GroupController, GroupError, GroupResult, LightGroup,
};

use crate::controller::{format_group_control_id, parse_group_control_id, MatterDeviceRegistry};
use crate::lifecycle::parse_device_id;
use crate::transport::{MatterGroup, MatterGroupMember, MatterTransport};

const FIRST_DETERMINISTIC_GROUP_ID: u16 = 0x8000;

/// `GroupController` implementation for the local Matter fabric.
pub struct MatterGroupController {
    transport: Arc<dyn MatterTransport>,
    registry: Arc<Mutex<MatterDeviceRegistry>>,
    groups: Mutex<Vec<LightGroup>>,
}

impl MatterGroupController {
    /// Create a Matter group controller backed by the same registry used for
    /// light control dispatch.
    pub fn new(
        transport: Arc<dyn MatterTransport>,
        registry: Arc<Mutex<MatterDeviceRegistry>>,
    ) -> Self {
        Self {
            transport,
            registry,
            groups: Mutex::new(Vec::new()),
        }
    }

    fn matter_members(device_ids: &[String]) -> Vec<MatterGroupMember> {
        let mut members: Vec<MatterGroupMember> = device_ids
            .iter()
            .filter_map(|device_id| {
                let (node_id, endpoint) = parse_device_id(device_id)?;
                Some(MatterGroupMember { node_id, endpoint })
            })
            .collect();
        members.sort_by_key(|member| (member.node_id, member.endpoint));
        members.dedup_by_key(|member| (member.node_id, member.endpoint));
        members
    }

    fn group_id_for_area(area_id: &str, allocated: &mut HashSet<u16>) -> u16 {
        let mut hash = 0xcbf29ce484222325u64;
        for byte in area_id.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }

        let mut group_id = FIRST_DETERMINISTIC_GROUP_ID | (hash as u16 & 0x7fff);
        if group_id == 0 {
            group_id = FIRST_DETERMINISTIC_GROUP_ID;
        }
        while allocated.contains(&group_id) {
            group_id = group_id.wrapping_add(1);
            if group_id == 0 {
                group_id = FIRST_DETERMINISTIC_GROUP_ID;
            }
        }
        allocated.insert(group_id);
        group_id
    }

    fn cache_groups(&self, groups: Vec<LightGroup>) {
        if let Ok(mut cached) = self.groups.lock() {
            *cached = groups;
        }
    }

    fn format_members(members: &[MatterGroupMember]) -> String {
        members
            .iter()
            .map(|member| format!("matter-{}:{}", member.node_id, member.endpoint))
            .collect::<Vec<_>>()
            .join(",")
    }
}

#[async_trait]
impl GroupController for MatterGroupController {
    async fn sync_groups(&self) -> GroupResult<Vec<LightGroup>> {
        let mut allocated = HashSet::new();
        let mut planned = Vec::new();

        {
            let registry = self.registry.lock().map_err(|error| {
                GroupError::SyncFailed(format!("Failed to lock Matter registry: {}", error))
            })?;
            let mut rooms = registry.rooms();
            rooms.sort_by(|left, right| left.id.cmp(&right.id));

            for room in rooms {
                let device_ids = registry.get_light_entities(&room.id);
                let members = Self::matter_members(&device_ids);
                if members.len() < 2 {
                    continue;
                }

                let existing_group_id = registry
                    .get_grouped_light_id(&room.id)
                    .and_then(|control_id| parse_group_control_id(&control_id));
                let group_id = match existing_group_id {
                    Some(group_id) if !allocated.contains(&group_id) => {
                        allocated.insert(group_id);
                        group_id
                    }
                    _ => Self::group_id_for_area(&room.id, &mut allocated),
                };

                let matter_group_name = group_name_for_area(&room.name);
                let control_id = format_group_control_id(group_id);
                let matter_group = MatterGroup {
                    group_id,
                    name: matter_group_name,
                    members,
                };
                let light_group = LightGroup::with_members(
                    control_id.clone(),
                    room.name.clone(),
                    room.id.clone(),
                    control_id,
                    device_ids,
                );
                planned.push((matter_group, light_group));
            }
        }

        for (matter_group, _) in &planned {
            info!(
                target: "sys",
                "Matter group sync: configuring group_id={} control_id={} name={} members={} endpoints=[{}]",
                matter_group.group_id,
                format_group_control_id(matter_group.group_id),
                matter_group.name,
                matter_group.members.len(),
                Self::format_members(&matter_group.members)
            );
            if let Err(error) = self.transport.configure_group(matter_group) {
                warn!(
                    target: "sys",
                    "Matter group sync failed: group_id={} control_id={} members={} error={}",
                    matter_group.group_id,
                    format_group_control_id(matter_group.group_id),
                    matter_group.members.len(),
                    error
                );
                return Err(GroupError::Communication(error.to_string()));
            }
            info!(
                target: "sys",
                "Matter group sync: configured group_id={} control_id={} members={}",
                matter_group.group_id,
                format_group_control_id(matter_group.group_id),
                matter_group.members.len()
            );
        }

        {
            let mut registry = self.registry.lock().map_err(|error| {
                GroupError::SyncFailed(format!("Failed to lock Matter registry: {}", error))
            })?;
            for (_, group) in &planned {
                registry.upsert_room(
                    &group.area_id,
                    &group.name,
                    &group.control_id,
                    &group.members,
                );
                info!(
                    target: "sys",
                    "Matter group sync: registered room={} control_id={} group_id={} members={}",
                    group.area_id,
                    group.control_id,
                    parse_group_control_id(&group.control_id).unwrap_or_default(),
                    group.members.len()
                );
            }
        }

        let groups: Vec<LightGroup> = planned
            .into_iter()
            .map(|(_, light_group)| light_group)
            .collect();
        self.cache_groups(groups.clone());
        info!(
            target: "sys",
            "Matter group sync complete: groups={}",
            groups.len()
        );
        Ok(groups)
    }

    async fn get_group_for_area(&self, area_id: &str) -> Option<LightGroup> {
        self.groups.lock().ok().and_then(|groups| {
            groups
                .iter()
                .find(|group| group.area_id == area_id)
                .cloned()
        })
    }

    async fn get_all_groups(&self) -> Vec<LightGroup> {
        self.groups
            .lock()
            .map(|groups| groups.clone())
            .unwrap_or_default()
    }

    async fn is_available(&self) -> bool {
        self.transport.list_devices().is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    use crate::test_support::{RecordedOperation, SpyTransport};

    fn block_on<F: Future>(mut future: F) -> F::Output {
        fn raw_waker() -> RawWaker {
            RawWaker::new(
                std::ptr::null(),
                &RawWakerVTable::new(|_| raw_waker(), |_| {}, |_| {}, |_| {}),
            )
        }

        // SAFETY: The future is not moved after pinning.
        let mut future = unsafe { Pin::new_unchecked(&mut future) };
        // SAFETY: The RawWaker above never dereferences the data pointer.
        let waker = unsafe { Waker::from_raw(raw_waker()) };
        let mut cx = Context::from_waker(&waker);
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("Mock future returned Pending"),
        }
    }

    fn registry_with_room() -> Arc<Mutex<MatterDeviceRegistry>> {
        let registry = Arc::new(Mutex::new(MatterDeviceRegistry::with_options(true)));
        registry.lock().unwrap().upsert_room(
            "kitchen",
            "Kitchen",
            "",
            &["matter-42".to_string(), "matter-43-2".to_string()],
        );
        registry
    }

    #[test]
    fn sync_groups_configures_matter_group_and_updates_registry_control_id() {
        let transport = Arc::new(SpyTransport::new());
        let registry = registry_with_room();
        let controller = MatterGroupController::new(transport.clone(), registry.clone());

        let groups = block_on(controller.sync_groups()).unwrap();

        assert_eq!(groups.len(), 1);
        let group = &groups[0];
        assert_eq!(group.area_id, "kitchen");
        assert!(parse_group_control_id(&group.control_id).is_some());
        assert_eq!(
            registry
                .lock()
                .unwrap()
                .get_grouped_light_id("kitchen")
                .as_deref(),
            Some(group.control_id.as_str())
        );

        let operations = transport.operations();
        assert_eq!(operations.len(), 1);
        match &operations[0] {
            RecordedOperation::ConfigureGroup { group } => {
                assert_eq!(
                    group.group_id,
                    parse_group_control_id(&groups[0].control_id).unwrap()
                );
                assert_eq!(
                    group.members,
                    vec![
                        MatterGroupMember {
                            node_id: 42,
                            endpoint: 1,
                        },
                        MatterGroupMember {
                            node_id: 43,
                            endpoint: 2,
                        },
                    ]
                );
            }
            other => panic!("expected ConfigureGroup, got {other:?}"),
        }
    }

    #[test]
    fn sync_groups_skips_single_light_rooms() {
        let transport = Arc::new(SpyTransport::new());
        let registry = Arc::new(Mutex::new(MatterDeviceRegistry::with_options(true)));
        registry
            .lock()
            .unwrap()
            .upsert_room("desk", "Desk", "", &["matter-42".to_string()]);
        let controller = MatterGroupController::new(transport.clone(), registry);

        let groups = block_on(controller.sync_groups()).unwrap();

        assert!(groups.is_empty());
        assert!(transport.operations().is_empty());
    }
}
