use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rhythm_core::{
    default_mode_configs, default_rhythm_profile, LightControlResult, LightController,
    LightNodeKind, LightingCommand, MockTimeProvider, NoOpScheduler, RestoredNodeState,
    RestoredRoomState, RhythmEngine, RhythmRuntime, Room, RoomProfileSettings, RuntimeConfig,
    RuntimeHandle, SimpleDeviceRegistry, SolarTime, SunTimes, RHYTHM_PROFILE_ID,
};

struct CountingController {
    turn_on_calls: AtomicUsize,
    turn_off_calls: AtomicUsize,
    lights_on: AtomicBool,
    commands: Mutex<Vec<(String, LightingCommand)>>,
}

#[tokio::test]
async fn direct_rhythm_engine_methods_cover_active_standby_mood_and_periodic_light_paths() {
    let controller = Arc::new(CountingController::new());
    let mut engine = RhythmEngine::with_profiles(
        controller.clone(),
        vec![default_rhythm_profile()],
        RHYTHM_PROFILE_ID,
        SolarTime::default_noon(),
    );
    engine.set_sun_times(SunTimes {
        sunrise: 6.0,
        sunset: 20.0,
        day_length: 14.0,
    });
    engine.clear_sun_times();
    engine.rooms_mut().add_room(Room::new("room-a", "Room A"));
    engine.set_light_profile_config(default_rhythm_profile());
    engine.set_mode_configs(default_mode_configs());
    assert!(engine.set_light_profile(RHYTHM_PROFILE_ID));
    assert!(!engine.available_profiles().is_empty());
    assert_eq!(engine.controller().name(), "counting");

    engine.turn_on("room-a", 14.0).await.unwrap();
    engine.step_up("room-a", 14.0).await.unwrap();
    engine.step_down("room-a", 14.0).await.unwrap();
    engine.dim_up("room-a", 14.0, None).await.unwrap();
    engine.dim_down("room-a", 14.0, Some(5.0)).await.unwrap();
    engine.set_brightness("room-a", 14.0, 35).await.unwrap();
    engine.set_time_offset("room-a", 14.0, 30.0).await.unwrap();
    engine.reset("room-a", 14.0).await.unwrap();

    if let Some(room) = engine.rooms_mut().get_mut("room-a") {
        room.standby_enabled = true;
    }
    engine.turn_off("room-a", 14.0).await.unwrap();
    engine.set_time_offset("room-a", 14.0, 45.0).await.unwrap();
    engine.soft_off_tick("room-a", 14.0).await.unwrap();
    engine.mood_tick("room-a", 14.0).await.unwrap();

    let mood_tick = engine.periodic_tick_single_room("room-a", 14.0).await;
    assert!(matches!(
        mood_tick,
        rhythm_core::PeriodicTickResult::Skipped
    ));

    engine.reset("room-a", 14.0).await.unwrap();
    controller.lights_on.store(true, Ordering::SeqCst);
    let periodic = engine.periodic_tick_node("room-a", "room-a", 14.0).await;
    assert!(matches!(
        periodic,
        rhythm_core::PeriodicTickResult::Updated | rhythm_core::PeriodicTickResult::Skipped
    ));
    let (_updated, errors) = engine.periodic_update(14.0).await;
    assert!(errors.is_empty());

    engine.lights_off("room-a", Some(120)).await.unwrap();
    assert!(controller.turn_on_count() >= 8);
    assert!(controller.turn_off_count() >= 1);
}

impl CountingController {
    fn new() -> Self {
        Self {
            turn_on_calls: AtomicUsize::new(0),
            turn_off_calls: AtomicUsize::new(0),
            lights_on: AtomicBool::new(true),
            commands: Mutex::new(Vec::new()),
        }
    }

    fn turn_on_count(&self) -> usize {
        self.turn_on_calls.load(Ordering::SeqCst)
    }

    fn turn_off_count(&self) -> usize {
        self.turn_off_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl LightController for CountingController {
    async fn turn_on(&self, room_id: &str, command: LightingCommand) -> LightControlResult<()> {
        self.turn_on_calls.fetch_add(1, Ordering::SeqCst);
        self.commands
            .lock()
            .unwrap()
            .push((room_id.to_string(), command));
        Ok(())
    }

    async fn turn_off(
        &self,
        _room_id: &str,
        _transition_ms: Option<u32>,
    ) -> LightControlResult<()> {
        self.turn_off_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        Ok(Vec::new())
    }

    async fn is_connected(&self) -> bool {
        true
    }

    async fn any_lights_on(&self, _room_id: &str) -> LightControlResult<bool> {
        Ok(self.lights_on.load(Ordering::SeqCst))
    }

    fn name(&self) -> &str {
        "counting"
    }
}

fn runtime_with_controller(
    controller: Arc<CountingController>,
) -> RhythmRuntime<CountingController, MockTimeProvider, NoOpScheduler, SimpleDeviceRegistry> {
    RhythmRuntime::new(
        controller,
        MockTimeProvider::new(14.0, 172, 2026),
        NoOpScheduler::new(),
        SimpleDeviceRegistry::new(),
        RuntimeConfig::default(),
    )
}

#[test]
fn runtime_handle_concrete_methods_preserve_node_and_light_behavior_contracts() {
    let controller = Arc::new(CountingController::new());
    let runtime = runtime_with_controller(controller.clone());

    RuntimeHandle::sync_rooms(&runtime).unwrap();
    RuntimeHandle::set_solar(&runtime, SolarTime::new(12.25, 35.0, 172)).unwrap();
    RuntimeHandle::set_sun_times(
        &runtime,
        SunTimes {
            sunrise: 6.0,
            sunset: 20.0,
            day_length: 14.0,
        },
    )
    .unwrap();
    RuntimeHandle::clear_sun_times(&runtime).unwrap();

    RuntimeHandle::add_room(&runtime, "room-a", "Room A");
    RuntimeHandle::add_node(
        &runtime,
        "light-a",
        "Light A",
        LightNodeKind::LightDevice,
        Some("room-a".to_string()),
    );
    RuntimeHandle::restore_room_state(
        &runtime,
        "room-a",
        RestoredRoomState {
            rhythm_enabled: true,
            disabled: false,
            time_offset_minutes: 15.0,
            brightness_offset: -5.0,
            soft_off: false,
            mood_active: false,
            standby_enabled: true,
            hard_off: false,
            profile_settings: RoomProfileSettings::default(),
        },
    );
    RuntimeHandle::restore_node_state(
        &runtime,
        "light-a",
        RestoredNodeState {
            rhythm_enabled: true,
            disabled: false,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
            soft_off: false,
            mood_active: false,
            standby_enabled: true,
            hard_off: false,
            profile_settings: RoomProfileSettings::default(),
        },
    );

    let room_snapshot = RuntimeHandle::engine_room_snapshot(&runtime, "room-a").unwrap();
    let restored_room = RestoredRoomState::from(&room_snapshot);
    assert_eq!(restored_room.time_offset_minutes, 15.0);
    let node_snapshot = RuntimeHandle::engine_node_snapshot(&runtime, "light-a").unwrap();
    let restored_node = RestoredNodeState::from(&node_snapshot);
    assert!(restored_node.rhythm_enabled);
    assert_eq!(RuntimeHandle::engine_all_room_snapshots(&runtime).len(), 1);
    assert_eq!(RuntimeHandle::engine_all_node_snapshots(&runtime).len(), 2);
    assert_eq!(
        RuntimeHandle::engine_effective_node_snapshot(&runtime, "light-a")
            .unwrap()
            .brightness_offset,
        -5.0
    );
    assert_eq!(
        RuntimeHandle::engine_all_effective_node_snapshots(&runtime).len(),
        2
    );

    RuntimeHandle::set_light_profile_config(&runtime, default_rhythm_profile()).unwrap();
    RuntimeHandle::set_mode_configs(&runtime, default_mode_configs()).unwrap();
    assert!(RuntimeHandle::set_light_profile(
        &runtime,
        RHYTHM_PROFILE_ID
    ));
    assert_eq!(
        RuntimeHandle::active_light_profile_id(&runtime),
        RHYTHM_PROFILE_ID
    );
    assert!(!RuntimeHandle::available_light_profiles(&runtime).is_empty());
    let _ = RuntimeHandle::is_power_save(&runtime);
    let _ = RuntimeHandle::set_power_save(&runtime, true);
    assert!(RuntimeHandle::is_power_save(&runtime));
    assert!(RuntimeHandle::idle_brightness(&runtime) >= 1);
    assert!(RuntimeHandle::any_lights_on(&runtime, "room-a").unwrap());

    RuntimeHandle::turn_on_room(&runtime, "room-a").unwrap();
    RuntimeHandle::apply_room_command(&runtime, "room-a", LightingCommand::new(44, 3900)).unwrap();
    RuntimeHandle::set_room_brightness(&runtime, "room-a", 35).unwrap();
    RuntimeHandle::set_room_time_offset(&runtime, "room-a", 30.0).unwrap();
    let _ = RuntimeHandle::set_room_curve_color_temperature(&runtime, "room-a", 3600, true);
    RuntimeHandle::dim_room(&runtime, "room-a", 0.5).unwrap();
    RuntimeHandle::soft_off_tick_room(&runtime, "room-a").unwrap();
    let _ = RuntimeHandle::mood_tick_room(&runtime, "room-a");
    RuntimeHandle::lights_off_room(&runtime, "room-a", Some(120)).unwrap();

    assert!(controller.turn_on_count() >= 4);
    assert_eq!(controller.turn_off_count(), 1);

    RuntimeHandle::remove_node(&runtime, "light-a");
    RuntimeHandle::remove_room(&runtime, "room-a");
    assert!(RuntimeHandle::engine_node_snapshot(&runtime, "light-a").is_none());
}
