//! Integration flow tests for rhythm-matter.
//!
//! Tests the full pipeline: engine action → MatterLightController → SpyTransport.
//! Verifies Matter-specific formatting (node IDs, cluster commands, level/mireds).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rhythm_core::runtime::handle::RuntimeHandle;
use rhythm_core::runtime::orchestrator::RhythmRuntime;
use rhythm_core::runtime::registry::SimpleDeviceRegistry;
use rhythm_core::runtime::scheduler::NoOpScheduler;
use rhythm_core::runtime::time::MockTimeProvider;
use rhythm_core::runtime::RuntimeConfig;
use rhythm_core::InputEvent;
use rhythm_devices::{LightCapabilities, LightType};

use rhythm_matter::controller::MatterLightController;
use rhythm_matter::hub_state::MatterHubData;
use rhythm_matter::test_support::SpyTransport;

use rhythm_os::hub::HubEvent;
use rhythm_os::registry::HubDeviceRegistry;

/// Create a testable Matter pipeline: spy transport + controller + runtime + registry.
fn make_matter_pipeline() -> (
    Arc<dyn RuntimeHandle>,
    Arc<Mutex<HubDeviceRegistry>>,
    Arc<SpyTransport>,
) {
    let spy = Arc::new(SpyTransport::new());
    let registry = Arc::new(Mutex::new(HubDeviceRegistry::with_options(true)));

    // Room with Matter light devices
    registry
        .lock()
        .unwrap()
        .upsert_room("room1", "Kitchen", "room1", &["matter-42".to_string()]);
    registry
        .lock()
        .unwrap()
        .set_area_lights("room1", vec!["matter-42".to_string()]);

    let (event_tx, _event_rx) = std::sync::mpsc::channel::<HubEvent>();

    let hub_data = Arc::new(MatterHubData {
        #[cfg(feature = "desktop")]
        transport: std::sync::OnceLock::new(),
        registry: registry.clone(),
        fabric_id: "test".to_string(),
        commissioned: std::sync::Mutex::new(Vec::new()),
        device_caps: std::sync::Mutex::new(std::collections::HashMap::new()),
        event_tx,
    });

    let controller = MatterLightController::new(spy.clone(), hub_data);

    let runtime = RhythmRuntime::new(
        controller,
        MockTimeProvider::new(14.0, 172, 2026),
        NoOpScheduler::new(),
        SimpleDeviceRegistry::new(),
        RuntimeConfig::default(),
    );
    runtime.add_room("room1", "Kitchen");

    (Arc::new(runtime), registry, spy)
}

// ============================================================================
// Tests
// ============================================================================

#[test]
fn engine_on_press_sends_matter_cluster_commands() {
    let (runtime, _registry, spy) = make_matter_pipeline();

    // Simulate button press → engine processes → controller sends commands
    let input = InputEvent::new("room1", rhythm_core::ButtonAction::OnPress);
    runtime.handle_event(&input).unwrap();

    let calls = spy.commands();
    // Should have at least 2 commands: level + color temperature
    assert!(
        calls.len() >= 2,
        "Expected at least 2 cluster commands (level + CT), got {}",
        calls.len()
    );

    // All commands should target node 42
    for call in &calls {
        assert_eq!(call.node_id, 42, "Command should target node 42");
        assert_eq!(call.endpoint, 1, "Command should target endpoint 1");
    }

    // Verify cluster IDs
    let clusters: Vec<u16> = calls.iter().map(|c| c.cluster).collect();
    assert!(
        clusters.contains(&0x0008),
        "Expected Level Control cluster (0x0008)"
    );
    assert!(
        clusters.contains(&0x0300),
        "Expected Color Control cluster (0x0300)"
    );
}

#[test]
fn engine_reset_sends_adaptive_values() {
    let (runtime, _, spy) = make_matter_pipeline();

    // Reset sends current adaptive curve values
    let input = InputEvent::new("room1", rhythm_core::ButtonAction::Reset);
    runtime.handle_event(&input).unwrap();

    let calls = spy.commands();
    assert!(
        calls.len() >= 2,
        "Expected level + CT commands, got {}",
        calls.len()
    );

    // Level command should be MoveToLevelWithOnOff (0x04)
    let level_cmd = calls
        .iter()
        .find(|c| c.cluster == 0x0008)
        .expect("Expected Level Control command");
    assert_eq!(
        level_cmd.cmd_id, 0x04,
        "Expected MoveToLevelWithOnOff (0x04)"
    );

    // Color XY command should be MoveToColor (0x07)
    let color_cmd = calls
        .iter()
        .find(|c| c.cluster == 0x0300)
        .expect("Expected Color Control command");
    assert_eq!(color_cmd.cmd_id, 0x07, "Expected MoveToColor (0x07)");
}

#[test]
fn multiple_devices_in_room_all_receive_commands() {
    let spy = Arc::new(SpyTransport::new());
    let registry = Arc::new(Mutex::new(HubDeviceRegistry::with_options(true)));

    // Room with 3 Matter devices
    let device_ids = vec![
        "matter-10".to_string(),
        "matter-20".to_string(),
        "matter-30".to_string(),
    ];
    registry
        .lock()
        .unwrap()
        .upsert_room("room1", "Bedroom", "room1", &device_ids);
    registry
        .lock()
        .unwrap()
        .set_area_lights("room1", device_ids);

    let (event_tx, _) = std::sync::mpsc::channel();
    let hub_data = Arc::new(MatterHubData {
        #[cfg(feature = "desktop")]
        transport: std::sync::OnceLock::new(),
        registry: registry.clone(),
        fabric_id: "test".to_string(),
        commissioned: std::sync::Mutex::new(Vec::new()),
        device_caps: std::sync::Mutex::new(std::collections::HashMap::new()),
        event_tx,
    });

    let controller = MatterLightController::new(spy.clone(), hub_data);

    let runtime = RhythmRuntime::new(
        controller,
        MockTimeProvider::new(14.0, 172, 2026),
        NoOpScheduler::new(),
        SimpleDeviceRegistry::new(),
        RuntimeConfig::default(),
    );
    runtime.add_room("room1", "Bedroom");

    let runtime: Arc<dyn RuntimeHandle> = Arc::new(runtime);
    let input = InputEvent::new("room1", rhythm_core::ButtonAction::OnPress);
    runtime.handle_event(&input).unwrap();

    let calls = spy.commands();
    let node_ids: Vec<u64> = calls.iter().map(|c| c.node_id).collect();

    // All 3 devices should receive commands
    assert!(node_ids.contains(&10), "Node 10 should get commands");
    assert!(node_ids.contains(&20), "Node 20 should get commands");
    assert!(node_ids.contains(&30), "Node 30 should get commands");
}

// ============================================================================
// Capability-aware command adaptation tests
// ============================================================================

/// Helper: build a pipeline with per-device capabilities.
fn make_pipeline_with_caps(
    devices: &[(&str, LightCapabilities)],
) -> (Arc<dyn RuntimeHandle>, Arc<SpyTransport>) {
    let spy = Arc::new(SpyTransport::new());
    let registry = Arc::new(Mutex::new(HubDeviceRegistry::with_options(true)));

    let device_ids: Vec<String> = devices.iter().map(|(id, _)| id.to_string()).collect();
    registry
        .lock()
        .unwrap()
        .upsert_room("room1", "Test Room", "room1", &device_ids);
    registry
        .lock()
        .unwrap()
        .set_area_lights("room1", device_ids);

    let mut caps_map = HashMap::new();
    for (id, caps) in devices {
        caps_map.insert(id.to_string(), caps.clone());
    }

    let (event_tx, _) = std::sync::mpsc::channel();
    let hub_data = Arc::new(MatterHubData {
        registry: registry.clone(),
        fabric_id: "test".to_string(),
        commissioned: std::sync::Mutex::new(Vec::new()),
        device_caps: Mutex::new(caps_map),
        event_tx,
    });

    let controller = MatterLightController::new(spy.clone(), hub_data);

    let runtime = RhythmRuntime::new(
        controller,
        MockTimeProvider::new(14.0, 172, 2026),
        NoOpScheduler::new(),
        SimpleDeviceRegistry::new(),
        RuntimeConfig::default(),
    );
    runtime.add_room("room1", "Test Room");

    (Arc::new(runtime), spy)
}

#[test]
fn dimmable_device_gets_level_only_no_color_temp() {
    let (runtime, spy) = make_pipeline_with_caps(&[(
        "matter-42",
        LightCapabilities::defaults_for(LightType::Dimmable),
    )]);

    let input = InputEvent::new("room1", rhythm_core::ButtonAction::OnPress);
    runtime.handle_event(&input).unwrap();

    let calls = spy.commands();

    // Should only get Level Control commands, no Color Control
    let has_level = calls.iter().any(|c| c.cluster == 0x0008);
    let has_color = calls.iter().any(|c| c.cluster == 0x0300);

    assert!(
        has_level,
        "Dimmable device should get Level Control command"
    );
    assert!(
        !has_color,
        "Dimmable device should NOT get Color Control command"
    );
}

#[test]
fn mixed_room_each_device_gets_appropriate_commands() {
    let (runtime, spy) = make_pipeline_with_caps(&[
        (
            "matter-10",
            LightCapabilities::defaults_for(LightType::ExtendedColor),
        ),
        (
            "matter-20",
            LightCapabilities::defaults_for(LightType::Dimmable),
        ),
    ]);

    let input = InputEvent::new("room1", rhythm_core::ButtonAction::OnPress);
    runtime.handle_event(&input).unwrap();

    let calls = spy.commands();

    // Node 10 (ExtendedColor) should get both level AND color temp
    let node10_clusters: Vec<u16> = calls
        .iter()
        .filter(|c| c.node_id == 10)
        .map(|c| c.cluster)
        .collect();
    assert!(
        node10_clusters.contains(&0x0008),
        "ExtendedColor device should get Level Control"
    );
    assert!(
        node10_clusters.contains(&0x0300),
        "ExtendedColor device should get Color Control"
    );

    // Node 20 (Dimmable) should get level only
    let node20_clusters: Vec<u16> = calls
        .iter()
        .filter(|c| c.node_id == 20)
        .map(|c| c.cluster)
        .collect();
    assert!(
        node20_clusters.contains(&0x0008),
        "Dimmable device should get Level Control"
    );
    assert!(
        !node20_clusters.contains(&0x0300),
        "Dimmable device should NOT get Color Control"
    );
}

#[test]
fn on_off_device_gets_on_command_only() {
    let (runtime, spy) = make_pipeline_with_caps(&[(
        "matter-42",
        LightCapabilities::defaults_for(LightType::OnOff),
    )]);

    let input = InputEvent::new("room1", rhythm_core::ButtonAction::OnPress);
    runtime.handle_event(&input).unwrap();

    let calls = spy.commands();

    // Should get On/Off cluster On command, no Level or Color
    let has_on_off = calls.iter().any(|c| c.cluster == 0x0006);
    let has_level = calls.iter().any(|c| c.cluster == 0x0008);
    let has_color = calls.iter().any(|c| c.cluster == 0x0300);

    assert!(has_on_off, "OnOff device should get On/Off cluster command");
    assert!(!has_level, "OnOff device should NOT get Level Control");
    assert!(!has_color, "OnOff device should NOT get Color Control");
}

#[test]
fn unknown_device_falls_back_to_extended_color() {
    // No caps registered for this device — should fall back to ExtendedColor defaults
    let spy = Arc::new(SpyTransport::new());
    let registry = Arc::new(Mutex::new(HubDeviceRegistry::with_options(true)));
    registry
        .lock()
        .unwrap()
        .upsert_room("room1", "Room", "room1", &["matter-99".to_string()]);
    registry
        .lock()
        .unwrap()
        .set_area_lights("room1", vec!["matter-99".to_string()]);

    let (event_tx, _) = std::sync::mpsc::channel();
    let hub_data = Arc::new(MatterHubData {
        registry: registry.clone(),
        fabric_id: "test".to_string(),
        commissioned: std::sync::Mutex::new(Vec::new()),
        device_caps: Mutex::new(HashMap::new()), // Empty — no caps known
        event_tx,
    });

    let controller = MatterLightController::new(spy.clone(), hub_data);

    let runtime = RhythmRuntime::new(
        controller,
        MockTimeProvider::new(14.0, 172, 2026),
        NoOpScheduler::new(),
        SimpleDeviceRegistry::new(),
        RuntimeConfig::default(),
    );
    runtime.add_room("room1", "Room");

    let runtime: Arc<dyn RuntimeHandle> = Arc::new(runtime);
    let input = InputEvent::new("room1", rhythm_core::ButtonAction::OnPress);
    runtime.handle_event(&input).unwrap();

    let calls = spy.commands();

    // Fallback = ExtendedColor → should get both level and color temp
    let has_level = calls.iter().any(|c| c.cluster == 0x0008);
    let has_color = calls.iter().any(|c| c.cluster == 0x0300);

    assert!(
        has_level,
        "Unknown device should get Level Control (fallback)"
    );
    assert!(
        has_color,
        "Unknown device should get Color Control (fallback)"
    );
}
