use rhythm_core::{
    LightNodeKind, LightScheduleAssignment, ModeTransitionTime, RhythmMode, RoomManager,
    RoomProfileSettings, RoomScheduleConfig, RoomScheduleSource,
};

fn custom_room_schedule(wake: &str, sleep: &str) -> RoomScheduleConfig {
    RoomScheduleConfig {
        source: RoomScheduleSource::FollowTime,
        wake_time: ModeTransitionTime::parse(wake).unwrap(),
        sleep_time: ModeTransitionTime::parse(sleep).unwrap(),
    }
}

#[test]
fn scenario_named_schedule_cascades_through_room_to_light() {
    let mut topology = RoomManager::new();
    topology.add_node("floor", "Main Floor", LightNodeKind::Room, None);
    topology.add_node(
        "kitchen",
        "Kitchen",
        LightNodeKind::Room,
        Some("floor".into()),
    );
    topology.add_node(
        "pendant",
        "Pendant",
        LightNodeKind::LightDevice,
        Some("kitchen".into()),
    );
    topology.get_mut("floor").unwrap().profile_settings = RoomProfileSettings {
        light_schedule: Some(LightScheduleAssignment::Named {
            schedule_id: "indoor".into(),
            active_mode: RhythmMode::Sleep,
        }),
        ..Default::default()
    };

    let kitchen = topology.effective_state("kitchen").unwrap();
    let pendant = topology.effective_state("pendant").unwrap();

    assert_eq!(
        kitchen
            .profile_settings
            .light_schedule
            .as_ref()
            .and_then(LightScheduleAssignment::schedule_id),
        Some("indoor")
    );
    assert_eq!(
        pendant
            .profile_settings
            .light_schedule
            .as_ref()
            .and_then(LightScheduleAssignment::schedule_id),
        Some("indoor")
    );
    assert_eq!(
        pendant
            .profile_settings
            .schedule_mode(RhythmMode::Day, 12.0),
        RhythmMode::Sleep
    );
}

#[test]
fn scenario_room_custom_schedule_overrides_inherited_named_schedule_for_descendants() {
    let mut topology = RoomManager::new();
    topology.add_node("floor", "Main Floor", LightNodeKind::Room, None);
    topology.add_node("porch", "Porch", LightNodeKind::Room, Some("floor".into()));
    topology.add_node(
        "sconce",
        "Sconce",
        LightNodeKind::LightDevice,
        Some("porch".into()),
    );
    topology.get_mut("floor").unwrap().profile_settings = RoomProfileSettings {
        light_schedule: Some(LightScheduleAssignment::Named {
            schedule_id: "indoor".into(),
            active_mode: RhythmMode::Sleep,
        }),
        ..Default::default()
    };
    let porch_schedule = custom_room_schedule("06:00", "23:00");
    topology.get_mut("porch").unwrap().profile_settings = RoomProfileSettings {
        room_schedule: Some(porch_schedule),
        ..Default::default()
    };

    let porch = topology.effective_state("porch").unwrap();
    let sconce = topology.effective_state("sconce").unwrap();

    assert_eq!(porch.profile_settings.light_schedule, None);
    assert_eq!(porch.profile_settings.room_schedule, Some(porch_schedule));
    assert_eq!(sconce.profile_settings.light_schedule, None);
    assert_eq!(sconce.profile_settings.room_schedule, Some(porch_schedule));
    assert_eq!(
        sconce
            .profile_settings
            .schedule_mode(RhythmMode::Sleep, 12.0),
        RhythmMode::Day
    );
}

#[test]
fn scenario_explicit_unscheduled_room_blocks_parent_custom_schedule() {
    let mut topology = RoomManager::new();
    topology.add_node("floor", "Main Floor", LightNodeKind::Room, None);
    topology.add_node(
        "utility",
        "Utility Room",
        LightNodeKind::Room,
        Some("floor".into()),
    );
    topology.add_node(
        "utility-light",
        "Utility Light",
        LightNodeKind::LightDevice,
        Some("utility".into()),
    );
    topology.get_mut("floor").unwrap().profile_settings = RoomProfileSettings {
        room_schedule: Some(custom_room_schedule("06:00", "23:00")),
        ..Default::default()
    };
    topology.get_mut("utility").unwrap().profile_settings = RoomProfileSettings {
        light_schedule: Some(LightScheduleAssignment::Unscheduled {
            active_mode: RhythmMode::Sleep,
        }),
        ..Default::default()
    };

    let utility = topology.effective_state("utility").unwrap();
    let light = topology.effective_state("utility-light").unwrap();

    assert!(matches!(
        utility.profile_settings.light_schedule,
        Some(LightScheduleAssignment::Unscheduled { .. })
    ));
    assert_eq!(utility.profile_settings.room_schedule, None);
    assert!(matches!(
        light.profile_settings.light_schedule,
        Some(LightScheduleAssignment::Unscheduled { .. })
    ));
    assert_eq!(light.profile_settings.room_schedule, None);
    assert_eq!(
        light.profile_settings.schedule_mode(RhythmMode::Day, 12.0),
        RhythmMode::Sleep
    );
}
