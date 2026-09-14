//! A queued group tick must remain deliverable while reconnect fences grouped control.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rhythm_core::{CompositeController, HubDispatchTarget, LightController, LightingCommand};
use rhythm_hue::controller::HueLightController;
use rhythm_hue::test_support::{HueTransportCall, SpyHueTransport};
use rhythm_os::canonical::identity::{DiscoveredIdentity, HubKey};
use rhythm_os::canonical::registry::{CanonicalRegistry, ResolveResult};
use rhythm_os::hub::HubType;
use rhythm_os::registry::HubDeviceRegistry;
use rhythm_os::topology::RoomTopologyStore;

#[test]
fn queued_large_hue_group_tick_survives_direct_fallback_and_group_recovery() {
    let key = HubKey::new(HubType::new("hue"), "bridge");
    let native_ids: Vec<_> = (0..13).map(|index| format!("device-{index:02}")).collect();
    let mut topology = RoomTopologyStore::new();
    let room = topology.translate_or_create(
        &key,
        "native-room",
        "Large room",
        "group-light",
        &native_ids,
    );
    let mut canonical = CanonicalRegistry::new();
    let transport = Arc::new(SpyHueTransport::new());
    for native_id in &native_ids {
        let identity = DiscoveredIdentity {
            native_id: native_id.clone(),
            room_id: Some("native-room".to_string()),
            room_name: Some("Large room".to_string()),
            name: native_id.clone(),
            device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
            hardware_ids: Vec::new(),
            manufacturer: None,
            model: None,
        };
        let ResolveResult::Created { canonical_id } = canonical.resolve(&identity, &key, 1_000)
        else {
            panic!("synthetic light should be new")
        };
        assert!(topology.attach_device_user_override(&room, &canonical_id));
        transport.set_resource_response(
            &format!("device/{native_id}"),
            serde_json::json!({"data": [{"services": [{
                "rtype": "light", "rid": format!("light-{native_id}")
            }]}]}),
        );
    }
    let scheduled_node = topology
        .periodic_light_nodes(&canonical)
        .into_iter()
        .find(|node| node.source_node_id == room)
        .unwrap()
        .id;
    let registry = Arc::new(Mutex::new(HubDeviceRegistry::new()));
    registry
        .lock()
        .unwrap()
        .upsert_room("native-room", "Large room", "group-light", &native_ids);
    let controller = HueLightController::new(transport.clone(), "test-key".into(), registry);
    let composite = CompositeController::new();
    composite.register_controller(&key.to_string(), Arc::new(controller));
    let (outcome_tx, outcome_rx) = std::sync::mpsc::channel();
    composite.set_outcome_listener(Arc::new(move |outcome| {
        let _ = outcome_tx.send(outcome);
    }));

    // Reconnect temporarily suspends grouped writes. A tick already planned
    // with the old synthetic group identity now routes to all 13 devices.
    topology.set_external_room_topology_sync_enabled(&key, true);
    let routing = topology.composite_routing(&canonical);
    assert!(matches!(&routing[&scheduled_node][0].1,
        HubDispatchTarget::Devices { native_ids } if native_ids.len() == 13));
    composite.update_routing(routing);
    futures::executor::block_on(composite.turn_on(&scheduled_node, LightingCommand::new(40, 2700)))
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut delivered = Vec::new();
    while delivered.len() < native_ids.len() {
        let outcome = outcome_rx.recv_timeout(deadline.saturating_duration_since(Instant::now()));
        if outcome.is_err() {
            composite.remove_controller(&key.to_string());
        }
        let outcome = outcome.expect("direct fallback must complete within the bridge rate budget");
        assert!(outcome.status.is_success(), "{outcome:?}");
        delivered.push(outcome.target_label);
    }
    delivered.sort();
    assert_eq!(delivered, native_ids);
    assert_eq!(
        transport.set_grouped_light_count(),
        0,
        "fenced groups must not be written"
    );
    let mut light_ids = transport
        .set_light_calls()
        .into_iter()
        .map(|call| {
            let HueTransportCall::SetLight {
                light_id,
                on,
                brightness,
                ..
            } = call
            else {
                unreachable!()
            };
            assert!(on);
            assert_eq!(brightness, Some(40));
            light_id
        })
        .collect::<Vec<_>>();
    light_ids.sort();
    assert_eq!(
        light_ids,
        native_ids
            .iter()
            .map(|id| format!("light-{id}"))
            .collect::<Vec<_>>()
    );

    // Verified topology restores ordinary grouped commands without a restart.
    topology.set_external_grouped_dispatch_suspended(&key, false);
    composite.update_routing(topology.composite_routing(&canonical));
    futures::executor::block_on(composite.turn_off(&room, Some(600))).unwrap();
    let outcome = outcome_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(outcome.status.is_success());
    assert_eq!(transport.set_light_calls().len(), 13);
    assert!(matches!(transport.set_grouped_light_calls().as_slice(),
        [HueTransportCall::SetGroupedLight { grouped_light_id, on: false,
            fade_ms: Some(600), .. }] if grouped_light_id == "group-light"));
}
