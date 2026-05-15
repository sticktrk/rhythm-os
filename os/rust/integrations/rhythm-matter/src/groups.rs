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

/// Desired Matter group derived from Rhythm topology.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatterTopologyGroup {
    pub area_id: String,
    pub name: String,
    pub member_device_ids: Vec<String>,
}

#[derive(Debug, Clone)]
struct ExistingMatterGroup {
    area_id: String,
    name: String,
    control_id: String,
    group_id: u16,
    member_device_ids: Vec<String>,
    members: Vec<MatterGroupMember>,
}

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

    fn normalize_device_ids(device_ids: &[String]) -> Vec<String> {
        let mut normalized = device_ids.to_vec();
        normalized.sort();
        normalized.dedup();
        normalized
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

    fn existing_managed_groups(registry: &MatterDeviceRegistry) -> Vec<ExistingMatterGroup> {
        let mut existing = Vec::new();
        let mut rooms = registry.rooms();
        rooms.sort_by(|left, right| left.id.cmp(&right.id));

        for room in rooms {
            let Some(control_id) = registry.get_grouped_light_id(&room.id) else {
                continue;
            };
            let Some(group_id) = parse_group_control_id(&control_id) else {
                continue;
            };
            let member_device_ids =
                Self::normalize_device_ids(&registry.get_light_entities(&room.id));
            let members = Self::matter_members(&member_device_ids);
            existing.push(ExistingMatterGroup {
                area_id: room.id,
                name: room.name,
                control_id,
                group_id,
                member_device_ids,
                members,
            });
        }

        existing
    }

    fn topology_specs_from_registry(registry: &MatterDeviceRegistry) -> Vec<MatterTopologyGroup> {
        let mut specs = Vec::new();
        let mut rooms = registry.rooms();
        rooms.sort_by(|left, right| left.id.cmp(&right.id));

        for room in rooms {
            let member_device_ids =
                Self::normalize_device_ids(&registry.get_light_entities(&room.id));
            if Self::matter_members(&member_device_ids).len() < 2 {
                continue;
            }
            specs.push(MatterTopologyGroup {
                area_id: room.id,
                name: room.name,
                member_device_ids,
            });
        }

        specs
    }

    /// Synchronize Matter groups from topology-derived desired membership.
    pub fn sync_topology_groups(
        &self,
        specs: &[MatterTopologyGroup],
    ) -> GroupResult<Vec<LightGroup>> {
        let existing = {
            let registry = self.registry.lock().map_err(|error| {
                GroupError::SyncFailed(format!("Failed to lock Matter registry: {}", error))
            })?;
            Self::existing_managed_groups(&registry)
        };

        let mut allocated: HashSet<u16> = existing.iter().map(|group| group.group_id).collect();
        let mut planned = Vec::new();

        let mut desired_specs = specs.to_vec();
        desired_specs.sort_by(|left, right| left.area_id.cmp(&right.area_id));
        for spec in desired_specs {
            let device_ids = Self::normalize_device_ids(&spec.member_device_ids);
            let members = Self::matter_members(&device_ids);
            if members.len() < 2 {
                continue;
            }

            let existing_group_id = existing
                .iter()
                .find(|group| group.area_id == spec.area_id)
                .map(|group| group.group_id);
            let group_id = existing_group_id
                .unwrap_or_else(|| Self::group_id_for_area(&spec.area_id, &mut allocated));

            let matter_group_name = group_name_for_area(&spec.name);
            let control_id = format_group_control_id(group_id);
            let matter_group = MatterGroup {
                group_id,
                name: matter_group_name,
                members,
            };
            let light_group = LightGroup::with_members(
                control_id.clone(),
                spec.name.clone(),
                spec.area_id.clone(),
                control_id,
                device_ids,
            );
            planned.push((matter_group, light_group));
        }

        let desired_area_ids: HashSet<&str> = planned
            .iter()
            .map(|(_, group)| group.area_id.as_str())
            .collect();
        let stale_groups: Vec<ExistingMatterGroup> = existing
            .iter()
            .filter(|group| !desired_area_ids.contains(group.area_id.as_str()))
            .cloned()
            .collect();

        for (matter_group, light_group) in &planned {
            if let Some(previous) = existing
                .iter()
                .find(|group| group.area_id == light_group.area_id)
            {
                let stale_members = previous
                    .members
                    .iter()
                    .filter(|member| !matter_group.members.contains(member))
                    .cloned()
                    .collect::<Vec<_>>();
                if !stale_members.is_empty() {
                    info!(
                        target: "sys",
                        "Matter group sync: removing stale members group_id={} control_id={} room={} stale_members={} endpoints=[{}]",
                        previous.group_id,
                        previous.control_id,
                        previous.area_id,
                        stale_members.len(),
                        Self::format_members(&stale_members)
                    );
                    if let Err(error) = self
                        .transport
                        .remove_group(previous.group_id, &stale_members)
                    {
                        warn!(
                            target: "sys",
                            "Matter group stale member removal failed: group_id={} control_id={} room={} stale_members={} error={}",
                            previous.group_id,
                            previous.control_id,
                            previous.area_id,
                            stale_members.len(),
                            error
                        );
                        return Err(GroupError::Communication(error.to_string()));
                    }
                    info!(
                        target: "sys",
                        "Matter group sync: removed stale members group_id={} control_id={} room={} stale_members={}",
                        previous.group_id,
                        previous.control_id,
                        previous.area_id,
                        stale_members.len()
                    );
                }
            }

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

        for stale in &stale_groups {
            info!(
                target: "sys",
                "Matter group sync: removing stale group_id={} control_id={} room={} name={} members={} endpoints=[{}]",
                stale.group_id,
                stale.control_id,
                stale.area_id,
                stale.name,
                stale.members.len(),
                Self::format_members(&stale.members)
            );
            if let Err(error) = self.transport.remove_group(stale.group_id, &stale.members) {
                warn!(
                    target: "sys",
                    "Matter group stale removal failed: group_id={} control_id={} room={} members={} error={}",
                    stale.group_id,
                    stale.control_id,
                    stale.area_id,
                    stale.members.len(),
                    error
                );
            } else {
                info!(
                    target: "sys",
                    "Matter group sync: removed stale group_id={} control_id={} room={} members={}",
                    stale.group_id,
                    stale.control_id,
                    stale.area_id,
                    stale.members.len()
                );
            }
        }

        {
            let mut registry = self.registry.lock().map_err(|error| {
                GroupError::SyncFailed(format!("Failed to lock Matter registry: {}", error))
            })?;
            for stale in &stale_groups {
                registry.remove_room(&stale.area_id);
                info!(
                    target: "sys",
                    "Matter group sync: unregistered stale room={} control_id={} group_id={} members={}",
                    stale.area_id,
                    stale.control_id,
                    stale.group_id,
                    stale.member_device_ids.len()
                );
            }
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
}

#[async_trait]
impl GroupController for MatterGroupController {
    async fn sync_groups(&self) -> GroupResult<Vec<LightGroup>> {
        let specs = {
            let registry = self.registry.lock().map_err(|error| {
                GroupError::SyncFailed(format!("Failed to lock Matter registry: {}", error))
            })?;
            Self::topology_specs_from_registry(&registry)
        };
        self.sync_topology_groups(&specs)
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

    #[test]
    fn sync_topology_groups_configures_group_and_updates_registry() {
        let transport = Arc::new(SpyTransport::new());
        let registry = Arc::new(Mutex::new(MatterDeviceRegistry::with_options(true)));
        let controller = MatterGroupController::new(transport.clone(), registry.clone());

        let groups = controller
            .sync_topology_groups(&[MatterTopologyGroup {
                area_id: "bookcase".to_string(),
                name: "Bookcase".to_string(),
                member_device_ids: vec!["matter-103".to_string(), "matter-102".to_string()],
            }])
            .unwrap();

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].area_id, "bookcase");
        assert_eq!(
            groups[0].members,
            vec!["matter-102".to_string(), "matter-103".to_string()]
        );
        assert_eq!(
            registry
                .lock()
                .unwrap()
                .get_grouped_light_id("bookcase")
                .as_deref(),
            Some(groups[0].control_id.as_str())
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
                            node_id: 102,
                            endpoint: 1,
                        },
                        MatterGroupMember {
                            node_id: 103,
                            endpoint: 1,
                        },
                    ]
                );
            }
            other => panic!("expected ConfigureGroup, got {other:?}"),
        }
    }

    #[test]
    fn sync_topology_groups_reuses_existing_group_and_removes_stale_members() {
        let transport = Arc::new(SpyTransport::new());
        let registry = Arc::new(Mutex::new(MatterDeviceRegistry::with_options(true)));
        registry.lock().unwrap().upsert_room(
            "kitchen",
            "Kitchen",
            "matter-group-32769",
            &["matter-42".to_string(), "matter-43".to_string()],
        );
        let controller = MatterGroupController::new(transport.clone(), registry.clone());

        let groups = controller
            .sync_topology_groups(&[MatterTopologyGroup {
                area_id: "kitchen".to_string(),
                name: "Kitchen".to_string(),
                member_device_ids: vec!["matter-43".to_string(), "matter-44".to_string()],
            }])
            .unwrap();

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].control_id, "matter-group-32769");
        assert_eq!(
            groups[0].members,
            vec!["matter-43".to_string(), "matter-44".to_string()]
        );
        assert_eq!(
            registry.lock().unwrap().get_light_entities("kitchen"),
            vec!["matter-43".to_string(), "matter-44".to_string()]
        );

        let operations = transport.operations();
        assert_eq!(operations.len(), 2);
        match &operations[0] {
            RecordedOperation::RemoveGroup { group_id, members } => {
                assert_eq!(*group_id, 32769);
                assert_eq!(
                    members,
                    &vec![MatterGroupMember {
                        node_id: 42,
                        endpoint: 1,
                    }]
                );
            }
            other => panic!("expected stale-member RemoveGroup, got {other:?}"),
        }
        match &operations[1] {
            RecordedOperation::ConfigureGroup { group } => {
                assert_eq!(group.group_id, 32769);
                assert_eq!(
                    group.members,
                    vec![
                        MatterGroupMember {
                            node_id: 43,
                            endpoint: 1,
                        },
                        MatterGroupMember {
                            node_id: 44,
                            endpoint: 1,
                        },
                    ]
                );
            }
            other => panic!("expected ConfigureGroup, got {other:?}"),
        }
    }

    #[test]
    fn sync_topology_groups_reconfigures_existing_group_without_member_churn() {
        let transport = Arc::new(SpyTransport::new());
        let registry = Arc::new(Mutex::new(MatterDeviceRegistry::with_options(true)));
        registry.lock().unwrap().upsert_room(
            "kitchen",
            "Kitchen",
            "matter-group-32769",
            &["matter-42".to_string(), "matter-43".to_string()],
        );
        let controller = MatterGroupController::new(transport.clone(), registry);

        let groups = controller
            .sync_topology_groups(&[MatterTopologyGroup {
                area_id: "kitchen".to_string(),
                name: "Kitchen Island".to_string(),
                member_device_ids: vec!["matter-43".to_string(), "matter-42".to_string()],
            }])
            .unwrap();

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].control_id, "matter-group-32769");

        let operations = transport.operations();
        assert_eq!(operations.len(), 1);
        match &operations[0] {
            RecordedOperation::ConfigureGroup { group } => {
                assert_eq!(group.group_id, 32769);
                assert_eq!(group.name, group_name_for_area("Kitchen Island"));
                assert_eq!(
                    group.members,
                    vec![
                        MatterGroupMember {
                            node_id: 42,
                            endpoint: 1,
                        },
                        MatterGroupMember {
                            node_id: 43,
                            endpoint: 1,
                        },
                    ]
                );
            }
            other => panic!("expected ConfigureGroup, got {other:?}"),
        }
    }

    #[test]
    fn sync_topology_groups_removes_group_when_room_drops_below_two_members() {
        let transport = Arc::new(SpyTransport::new());
        let registry = Arc::new(Mutex::new(MatterDeviceRegistry::with_options(true)));
        registry.lock().unwrap().upsert_room(
            "kitchen",
            "Kitchen",
            "matter-group-32769",
            &["matter-42".to_string(), "matter-43".to_string()],
        );
        let controller = MatterGroupController::new(transport.clone(), registry.clone());

        let groups = controller
            .sync_topology_groups(&[MatterTopologyGroup {
                area_id: "kitchen".to_string(),
                name: "Kitchen".to_string(),
                member_device_ids: vec!["matter-42".to_string()],
            }])
            .unwrap();

        assert!(groups.is_empty());
        assert_eq!(
            registry.lock().unwrap().get_grouped_light_id("kitchen"),
            None
        );

        let operations = transport.operations();
        assert_eq!(operations.len(), 1);
        match &operations[0] {
            RecordedOperation::RemoveGroup { group_id, members } => {
                assert_eq!(*group_id, 32769);
                assert_eq!(
                    members,
                    &vec![
                        MatterGroupMember {
                            node_id: 42,
                            endpoint: 1,
                        },
                        MatterGroupMember {
                            node_id: 43,
                            endpoint: 1,
                        },
                    ]
                );
            }
            other => panic!("expected RemoveGroup, got {other:?}"),
        }
    }

    #[test]
    fn sync_topology_groups_removes_stale_generated_group() {
        let transport = Arc::new(SpyTransport::new());
        let registry = Arc::new(Mutex::new(MatterDeviceRegistry::with_options(true)));
        registry.lock().unwrap().upsert_room(
            "old-room",
            "Old Room",
            "matter-group-32769",
            &["matter-42".to_string(), "matter-43".to_string()],
        );
        let controller = MatterGroupController::new(transport.clone(), registry.clone());

        let groups = controller.sync_topology_groups(&[]).unwrap();

        assert!(groups.is_empty());
        assert_eq!(
            registry.lock().unwrap().get_grouped_light_id("old-room"),
            None
        );
        let operations = transport.operations();
        assert_eq!(operations.len(), 1);
        match &operations[0] {
            RecordedOperation::RemoveGroup { group_id, members } => {
                assert_eq!(*group_id, 32769);
                assert_eq!(
                    members,
                    &vec![
                        MatterGroupMember {
                            node_id: 42,
                            endpoint: 1,
                        },
                        MatterGroupMember {
                            node_id: 43,
                            endpoint: 1,
                        },
                    ]
                );
            }
            other => panic!("expected RemoveGroup, got {other:?}"),
        }
    }
}
