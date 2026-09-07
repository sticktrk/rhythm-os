use async_trait::async_trait;
use rhythm_core::{
    controller::{HubDispatchTarget, HubLightController, LightController},
    lighting::LightingCommand,
};
use rhythm_monster::{
    controller::{static_values, LightMonsterController},
    lan::LightTransport,
    LightError, LightProperty as P, LightResult,
};
use rhythm_os::registry::HubDeviceRegistry;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
struct Spy {
    writes: Mutex<Vec<Vec<(P, Value)>>>,
    fail: bool,
}
#[async_trait]
impl LightTransport for Spy {
    async fn read(&self, _: P) -> LightResult<Value> {
        Ok(json!(1))
    }
    async fn write(&self, v: Vec<(P, Value)>) -> LightResult<()> {
        self.writes.lock().unwrap().push(v);
        if self.fail {
            Err(LightError::Timeout)
        } else {
            Ok(())
        }
    }
}
#[tokio::test]
async fn maps_rhythm_output_to_static_color_and_isolates_target_failure() {
    let good = Arc::new(Spy {
        writes: Mutex::new(vec![]),
        fail: false,
    });
    let bad = Arc::new(Spy {
        writes: Mutex::new(vec![]),
        fail: true,
    });
    let mut devices: HashMap<String, Arc<dyn LightTransport>> = HashMap::new();
    devices.insert("good".into(), good.clone());
    devices.insert("bad".into(), bad);
    let mut registry = HubDeviceRegistry::new();
    registry.upsert_room("room", "Room", "room", &["good".into()]);
    registry.set_area_lights("room", vec!["good".into()]);
    let controller = LightMonsterController::new(Arc::new(Mutex::new(registry)), devices);
    let command = LightingCommand::new(38, 2700);
    controller.turn_on("room", command).await.unwrap();
    let first = good.writes.lock().unwrap()[0].clone();
    assert!(first.contains(&(P::Mode, json!("color"))));
    assert!(first.contains(&(P::ColorBrightness, json!(38))));
    let result = controller
        .turn_off_target(
            &HubDispatchTarget::Devices {
                native_ids: vec!["bad".into(), "good".into()],
            },
            None,
        )
        .await;
    assert!(result.is_err());
    assert_eq!(good.writes.lock().unwrap()[1], vec![(P::Power, json!(0))]);
}
#[test]
fn zero_brightness_is_off_and_no_effects_or_white_claims_are_emitted() {
    assert_eq!(
        static_values(&LightingCommand::new(0, 2700)),
        vec![(P::Power, json!(0))]
    );
    for (p, v) in static_values(&LightingCommand::new(255, 6500)) {
        p.validate(&v).unwrap();
        assert_ne!(p.name(), "color_temp");
    }
}
