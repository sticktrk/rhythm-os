use crate::{lan::LightTransport, LightError, LightProperty as P};
use async_trait::async_trait;
use rhythm_core::{
    controller::{
        HubDispatchTarget, HubLightController, LightControlError, LightControlResult,
        LightController,
    },
    lighting::LightingCommand,
    room::Room,
};
use rhythm_os::{controller_helpers::rooms_from_registry, registry::HubDeviceRegistry};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

/// Static RGB output. Kelvin commands use Rhythm's existing Kelvin-to-RGB
/// projection. No dedicated white channel, native fades, scenes or effects are
/// advertised. A transition request applies its final state immediately.
pub fn static_values(command: &LightingCommand) -> Vec<(P, Value)> {
    if command.brightness == 0 {
        return vec![(P::Power, json!(0))];
    }
    let rgb = command.rgb;
    let color = ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | (rgb.b as u32);
    vec![
        (P::Mode, json!("color")),
        (P::Color, json!(color)),
        (P::Saturation, json!(100)),
        (P::Brightness, json!(command.brightness.min(100))),
        (P::ColorBrightness, json!(command.brightness.min(100))),
        (P::Power, json!(1)),
    ]
}
pub struct LightMonsterController {
    registry: Arc<Mutex<HubDeviceRegistry>>,
    devices: HashMap<String, Arc<dyn LightTransport>>,
}
fn error(e: LightError) -> LightControlError {
    LightControlError::CommandFailed(e.to_string())
}
impl LightMonsterController {
    pub fn new(
        registry: Arc<Mutex<HubDeviceRegistry>>,
        devices: HashMap<String, Arc<dyn LightTransport>>,
    ) -> Self {
        Self { registry, devices }
    }
    fn targets(
        &self,
        target: &HubDispatchTarget,
    ) -> LightControlResult<Vec<LightControlResult<Arc<dyn LightTransport>>>> {
        let ids = match target {
            HubDispatchTarget::Devices { native_ids } => native_ids.clone(),
            HubDispatchTarget::Group { room_id, .. } => self
                .registry
                .lock()
                .map_err(|_| LightControlError::Internal("Monster registry unavailable".into()))?
                .get_light_entities(room_id),
        };
        if ids.is_empty() {
            return Err(LightControlError::RoomNotFound(
                "Monster target has no lights".into(),
            ));
        }
        Ok(ids
            .iter()
            .map(|id| {
                self.devices
                    .get(id)
                    .cloned()
                    .ok_or(LightControlError::AuthRequired)
            })
            .collect())
    }
    async fn apply(
        &self,
        target: &HubDispatchTarget,
        values: Vec<(P, Value)>,
    ) -> LightControlResult<()> {
        // Futures run concurrently: one offline target cannot prevent sibling
        // writes. Each device transport owns its own serialization and timeout.
        let targets = self.targets(target)?;
        let mut tasks = tokio::task::JoinSet::new();
        let mut failure = None;
        for transport in targets {
            let transport = match transport {
                Ok(transport) => transport,
                Err(e) => {
                    failure = Some(e);
                    continue;
                }
            };
            let values = values.clone();
            tasks.spawn(async move { transport.write(values).await });
        }
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => failure = Some(error(e)),
                Err(_) => failure = Some(error(LightError::Unavailable)),
            }
        }
        failure.map_or(Ok(()), Err)
    }
    fn group(room_id: &str) -> HubDispatchTarget {
        HubDispatchTarget::Group {
            room_id: room_id.into(),
            control_id: room_id.into(),
        }
    }
}
#[async_trait]
impl LightController for LightMonsterController {
    async fn turn_on(&self, room: &str, command: LightingCommand) -> LightControlResult<()> {
        self.apply(&Self::group(room), static_values(&command))
            .await
    }
    async fn turn_off(&self, room: &str, _transition: Option<u32>) -> LightControlResult<()> {
        self.apply(&Self::group(room), vec![(P::Power, json!(0))])
            .await
    }
    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        rooms_from_registry(&self.registry)
    }
    async fn is_connected(&self) -> bool {
        // Probe every device at once and answer on the first verified read, so
        // offline siblings (each bounded by the transport's own timeout) cannot
        // serialize into a minute-long stall.
        let mut probes = tokio::task::JoinSet::new();
        for device in self.devices.values() {
            let device = Arc::clone(device);
            probes.spawn(async move { device.read(P::Power).await.is_ok() });
        }
        while let Some(result) = probes.join_next().await {
            if result.unwrap_or(false) {
                return true;
            }
        }
        false
    }
    async fn any_lights_on(&self, room: &str) -> LightControlResult<bool> {
        self.any_lights_on_target(&Self::group(room)).await
    }
    fn name(&self) -> &str {
        "Monster"
    }
}
#[async_trait]
impl HubLightController for LightMonsterController {
    async fn turn_on_target(
        &self,
        target: &HubDispatchTarget,
        command: LightingCommand,
    ) -> LightControlResult<()> {
        self.apply(target, static_values(&command)).await
    }
    async fn turn_off_target(
        &self,
        target: &HubDispatchTarget,
        _transition: Option<u32>,
    ) -> LightControlResult<()> {
        self.apply(target, vec![(P::Power, json!(0))]).await
    }
    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        rooms_from_registry(&self.registry)
    }
    async fn is_connected(&self) -> bool {
        LightController::is_connected(self).await
    }
    async fn any_lights_on_target(&self, target: &HubDispatchTarget) -> LightControlResult<bool> {
        let mut failure = None;
        for device in self.targets(target)? {
            let device = match device {
                Ok(device) => device,
                Err(e) => {
                    failure = Some(e);
                    continue;
                }
            };
            match device.read(P::Power).await {
                Ok(value) if value == 1 => return Ok(true),
                Ok(value) if value == 0 => {}
                Ok(_) => failure = Some(error(LightError::Readback)),
                Err(e) => failure = Some(error(e)),
            }
        }
        if let Some(e) = failure {
            Err(e)
        } else {
            Ok(false)
        }
    }
    fn name(&self) -> &str {
        "Monster"
    }
}
