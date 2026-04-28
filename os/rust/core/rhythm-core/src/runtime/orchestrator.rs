//! RhythmRuntime orchestrator.
//!
//! This module provides the main `RhythmRuntime` struct that ties together
//! the RhythmEngine with scheduling, event handling, and persistence.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use tracing::{debug, info};

use crate::controller::LightController;
use crate::light_profile::LightProfileConfig;
use crate::primitives::{ManualActionPlan, ManualDispatchPlan, RhythmEngine};
use crate::room::ModeConfig;
use crate::solar::{SolarTime, SunTimes};

use crate::runtime::config::RuntimeConfig;
use crate::runtime::error::{RuntimeError, RuntimeResult};
use crate::runtime::events::{ButtonAction, InputEvent};
use crate::runtime::registry::DeviceRegistry;
use crate::runtime::scheduler::Scheduler;
use crate::runtime::time::TimeProvider;

/// The main Rhythm OS runtime.
///
/// This struct orchestrates the RhythmEngine with scheduling, event handling,
/// and solar midnight reset logic. It's generic over the platform-specific
/// components (controller, time provider, scheduler).
///
/// # Type Parameters
///
/// * `C` - The light controller implementation
/// * `T` - The time provider implementation
/// * `S` - The scheduler implementation
/// * `R` - The device registry implementation
pub struct RhythmRuntime<C, T, S, R>
where
    C: LightController + Send + Sync + 'static,
    T: TimeProvider + Send + Sync + 'static,
    S: Scheduler + Send + Sync + 'static,
    R: DeviceRegistry + Send + Sync + 'static,
{
    /// The underlying RhythmEngine (wrapped in RwLock for thread-safe access).
    engine: Arc<RwLock<RhythmEngine<Arc<C>>>>,

    /// Shared controller handle kept outside the engine so background work
    /// can dispatch I/O without holding the engine lock.
    controller: Arc<C>,

    /// Per-target dispatch gates. These serialize same-target controller I/O
    /// while still allowing unrelated targets to proceed independently.
    dispatch_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,

    /// Time provider for getting current time.
    time_provider: Arc<T>,

    /// Scheduler for periodic tasks (retained for `S` generic; see deferred cleanup).
    #[allow(dead_code)]
    scheduler: Arc<S>,

    /// Device registry for device-to-room mapping.
    device_registry: Arc<RwLock<R>>,

    /// Runtime configuration.
    config: RuntimeConfig,
}

impl<C, T, S, R> RhythmRuntime<C, T, S, R>
where
    C: LightController + Send + Sync + 'static,
    T: TimeProvider + Send + Sync + 'static,
    S: Scheduler + Send + Sync + 'static,
    R: DeviceRegistry + Send + Sync + 'static,
{
    /// Create a new RhythmRuntime.
    ///
    /// # Arguments
    ///
    /// * `controller` - The light controller
    /// * `time_provider` - The time provider
    /// * `scheduler` - The scheduler
    /// * `device_registry` - The device registry
    /// * `config` - Runtime configuration
    pub fn new(
        controller: Arc<C>,
        time_provider: T,
        scheduler: S,
        device_registry: R,
        config: RuntimeConfig,
    ) -> Self {
        let engine = RhythmEngine::new(controller.clone());

        Self {
            engine: Arc::new(RwLock::new(engine)),
            controller,
            dispatch_locks: Mutex::new(HashMap::new()),
            time_provider: Arc::new(time_provider),
            scheduler: Arc::new(scheduler),
            device_registry: Arc::new(RwLock::new(device_registry)),
            config,
        }
    }

    /// Get a reference to the engine (for direct access if needed).
    pub fn engine(&self) -> &Arc<RwLock<RhythmEngine<Arc<C>>>> {
        &self.engine
    }

    /// Get a cloned light-controller handle.
    pub(crate) fn controller(&self) -> Arc<C> {
        self.controller.clone()
    }

    /// Get or create the per-target dispatch gate.
    pub(crate) fn dispatch_lock(&self, target_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self
            .dispatch_locks
            .lock()
            .expect("dispatch lock map poisoned");
        locks
            .entry(target_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Get a reference to the device registry.
    pub fn device_registry(&self) -> &Arc<RwLock<R>> {
        &self.device_registry
    }

    /// Get the current hour from the time provider.
    pub fn current_hour(&self) -> f32 {
        self.time_provider.current_hour()
    }

    /// Get the runtime configuration.
    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    /// Update the runtime configuration.
    pub fn set_config(&mut self, config: RuntimeConfig) {
        self.config = config;
    }

    /// Set or replace a light profile configuration on the engine.
    pub fn set_light_profile_config(&self, config: LightProfileConfig) -> RuntimeResult<()> {
        let mut engine = self
            .engine
            .write()
            .map_err(|e| RuntimeError::Internal(format!("Failed to lock engine: {}", e)))?;
        let config_id = config.id.clone();
        if !engine.set_light_profile_config(config.clone()) {
            debug!("Runtime registering custom light profile '{}'", config_id);
            engine.profile_registry_mut().register_config(config);
        }
        Ok(())
    }

    /// Replace the mode/state profile mappings on the engine.
    pub fn set_mode_configs<I>(&self, configs: I) -> RuntimeResult<()>
    where
        I: IntoIterator<Item = ModeConfig>,
    {
        let configs: Vec<_> = configs.into_iter().collect();
        let mut engine = self
            .engine
            .write()
            .map_err(|e| RuntimeError::Internal(format!("Failed to lock engine: {}", e)))?;
        engine.set_mode_configs(configs.iter().cloned());
        debug!("Runtime mode configs updated: {} entries", configs.len());
        Ok(())
    }

    /// Set the solar time on the engine.
    pub fn set_solar(&self, solar: SolarTime) -> RuntimeResult<()> {
        let mut engine = self
            .engine
            .write()
            .map_err(|e| RuntimeError::Internal(format!("Failed to lock engine: {}", e)))?;
        engine.set_solar(solar);
        Ok(())
    }

    /// Set the sun times on the engine.
    pub fn set_sun_times(&self, sun_times: SunTimes) -> RuntimeResult<()> {
        let mut engine = self
            .engine
            .write()
            .map_err(|e| RuntimeError::Internal(format!("Failed to lock engine: {}", e)))?;
        engine.set_sun_times(sun_times);
        Ok(())
    }

    /// Handle an input event.
    ///
    /// This routes the event to the appropriate RhythmEngine primitive.
    /// Returns true if the action resulted in lights being turned on.
    #[allow(clippy::await_holding_lock)] // Intentional: same-room I/O stays serialized; engine state is not held across awaits.
    pub async fn handle_event(&self, event: &InputEvent) -> RuntimeResult<bool> {
        let current_hour = self.current_hour();

        debug!(
            "Handling event: room={}, action={:?}, device={:?}",
            event.room_id, event.action, event.device_id
        );

        match event.action {
            ButtonAction::OnPress => info!("turn_on for {}", event.room_id),
            ButtonAction::Toggle => info!("toggle for {}", event.room_id),
            ButtonAction::OffPress => info!("turn_off for {}", event.room_id),
            ButtonAction::Reset => info!("reset for {}", event.room_id),
            ButtonAction::UpPress => info!("dim_up for {}", event.room_id),
            ButtonAction::DownPress => info!("dim_down for {}", event.room_id),
            ButtonAction::UpHold => info!("step_up for {}", event.room_id),
            ButtonAction::DownHold => info!("step_down for {}", event.room_id),
            ButtonAction::Stop => debug!("stop (ignored) for {}", event.room_id),
            ButtonAction::RhythmOn => info!("rhythm_on for {}", event.room_id),
            ButtonAction::RhythmOff => info!("rhythm_off for {}", event.room_id),
            ButtonAction::LightsOff => info!("lights_off for {}", event.room_id),
            ButtonAction::SleepOn => info!("sleep_on for {}", event.room_id),
            ButtonAction::SleepOff => info!("sleep_off for {}", event.room_id),
        }

        let dispatch_lock = self.dispatch_lock(&event.room_id);
        let _dispatch_guard = dispatch_lock
            .lock()
            .map_err(|e| RuntimeError::Internal(format!("Failed to lock dispatch gate: {}", e)))?;
        let mut lights_on = None;

        loop {
            let plan = {
                let mut engine = self
                    .engine
                    .write()
                    .map_err(|e| RuntimeError::Internal(format!("Failed to lock engine: {}", e)))?;
                engine.plan_button_action(&event.room_id, event.action, current_hour, lights_on)
            };

            match plan {
                ManualActionPlan::Noop { turned_on } => return Ok(turned_on),
                ManualActionPlan::RequiresLightCheck => {
                    lights_on = Some(self.controller.any_lights_on(&event.room_id).await?);
                }
                ManualActionPlan::Dispatch {
                    dispatch,
                    turned_on,
                } => {
                    self.dispatch_manual_plan(dispatch).await?;
                    return Ok(turned_on);
                }
            }
        }
    }

    async fn dispatch_manual_plan(&self, dispatch: ManualDispatchPlan) -> RuntimeResult<()> {
        match dispatch {
            ManualDispatchPlan::TurnOn {
                source_room_id,
                target_id,
                command,
            } => {
                log_manual_command_dispatch(&target_id, &command);
                self.controller.turn_on(&target_id, command.clone()).await?;
                let mut engine = self
                    .engine
                    .write()
                    .map_err(|e| RuntimeError::Internal(format!("Failed to lock engine: {}", e)))?;
                engine.record_turn_on_dispatch(&source_room_id, &target_id, command);
                Ok(())
            }
            ManualDispatchPlan::TurnOff {
                target_id,
                transition_ms,
            } => {
                info!(
                    target: "cmd",
                    "room_command_dispatch: room={} kind=turn_off transition_ms={:?}",
                    target_id,
                    transition_ms,
                );
                self.controller.turn_off(&target_id, transition_ms).await?;
                Ok(())
            }
        }
    }

    /// Handle a device event (from a button/remote).
    ///
    /// This looks up the room for the device and then handles the event.
    pub async fn handle_device_event(
        &self,
        device_id: &str,
        action: ButtonAction,
    ) -> RuntimeResult<bool> {
        // Look up the room for this device
        let room_id = {
            let registry = self.device_registry.read().map_err(|e| {
                RuntimeError::Internal(format!("Failed to lock device registry: {}", e))
            })?;
            registry.get_room_for_device(device_id)
        };

        let room_id = room_id.ok_or_else(|| RuntimeError::DeviceNotFound(device_id.to_string()))?;

        let event = InputEvent::with_device(&room_id, action, device_id);
        self.handle_event(&event).await
    }

    /// Sync rooms from the controller.
    #[allow(clippy::await_holding_lock)] // Intentional: sync RwLock with blocking engine ops
    pub async fn sync_rooms(&self) -> RuntimeResult<()> {
        let mut engine = self
            .engine
            .write()
            .map_err(|e| RuntimeError::Internal(format!("Failed to lock engine: {}", e)))?;
        engine.sync_rooms().await?;
        Ok(())
    }
}

fn log_manual_command_dispatch(target_id: &str, command: &crate::lighting::LightingCommand) {
    if command.is_direct_color {
        info!(
            target: "cmd",
            "room_command_dispatch: room={} bri={} rgb=({},{},{}) xy=({:.3},{:.3}) transition_ms={:?} direct_color=true",
            target_id,
            command.brightness,
            command.rgb.r,
            command.rgb.g,
            command.rgb.b,
            command.xy.x,
            command.xy.y,
            command.transition_ms,
        );
    } else {
        info!(
            target: "cmd",
            "room_command_dispatch: room={} bri={} kelvin={} rgb=({},{},{}) xy=({:.3},{:.3}) transition_ms={:?} direct_color=false",
            target_id,
            command.brightness,
            command.kelvin,
            command.rgb.r,
            command.rgb.g,
            command.rgb.b,
            command.xy.x,
            command.xy.y,
            command.transition_ms,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::NoOpController;
    use crate::runtime::registry::SimpleDeviceRegistry;
    use crate::runtime::scheduler::NoOpScheduler;
    use crate::runtime::time::MockTimeProvider;

    fn test_runtime(
    ) -> RhythmRuntime<NoOpController, MockTimeProvider, NoOpScheduler, SimpleDeviceRegistry> {
        RhythmRuntime::new(
            Arc::new(NoOpController::new()),
            MockTimeProvider::default(),
            NoOpScheduler::new(),
            SimpleDeviceRegistry::new(),
            RuntimeConfig::default(),
        )
    }

    #[tokio::test]
    async fn test_handle_event_on_press() {
        let runtime = test_runtime();
        let event = InputEvent::new("living_room", ButtonAction::OnPress);

        let result = runtime.handle_event(&event).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_event_step_up() {
        let runtime = test_runtime();
        let event = InputEvent::new("living_room", ButtonAction::UpPress);

        let result = runtime.handle_event(&event).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_device_event() {
        let runtime = test_runtime();

        // Register a device first
        {
            let mut registry = runtime.device_registry.write().unwrap();
            registry.register_device("switch1", "living_room");
        }

        let result = runtime
            .handle_device_event("switch1", ButtonAction::OnPress)
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_device_event_unknown() {
        let runtime = test_runtime();

        let result = runtime
            .handle_device_event("unknown_switch", ButtonAction::OnPress)
            .await;
        assert!(matches!(result, Err(RuntimeError::DeviceNotFound(_))));
    }

    #[tokio::test]
    async fn test_handle_event_toggle() {
        let runtime = test_runtime();
        let event = InputEvent::new("living_room", ButtonAction::Toggle);
        let result = runtime.handle_event(&event).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_event_off_press() {
        let runtime = test_runtime();
        let event = InputEvent::new("living_room", ButtonAction::OffPress);
        let result = runtime.handle_event(&event).await;
        assert!(result.is_ok());
        assert!(!result.unwrap()); // OffPress returns false (lights off)
    }

    #[tokio::test]
    async fn test_handle_event_reset() {
        let runtime = test_runtime();
        let event = InputEvent::new("living_room", ButtonAction::Reset);
        let result = runtime.handle_event(&event).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_event_down_press() {
        let runtime = test_runtime();
        let event = InputEvent::new("living_room", ButtonAction::DownPress);
        let result = runtime.handle_event(&event).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_event_up_hold() {
        let runtime = test_runtime();
        let event = InputEvent::new("living_room", ButtonAction::UpHold);
        let result = runtime.handle_event(&event).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_event_down_hold() {
        let runtime = test_runtime();
        let event = InputEvent::new("living_room", ButtonAction::DownHold);
        let result = runtime.handle_event(&event).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_event_stop() {
        let runtime = test_runtime();
        let event = InputEvent::new("living_room", ButtonAction::Stop);
        let result = runtime.handle_event(&event).await;
        assert!(result.is_ok());
        assert!(!result.unwrap()); // Stop returns false
    }

    #[tokio::test]
    async fn test_handle_event_rhythm_on() {
        let runtime = test_runtime();
        let event = InputEvent::new("living_room", ButtonAction::RhythmOn);
        let result = runtime.handle_event(&event).await;
        assert!(result.is_ok());
        assert!(!result.unwrap()); // RhythmOn returns false (no light change)
    }

    #[tokio::test]
    async fn test_handle_event_rhythm_off() {
        let runtime = test_runtime();
        let event = InputEvent::new("living_room", ButtonAction::RhythmOff);
        let result = runtime.handle_event(&event).await;
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    #[tokio::test]
    async fn test_handle_event_lights_off() {
        let runtime = test_runtime();
        let event = InputEvent::new("living_room", ButtonAction::LightsOff);
        let result = runtime.handle_event(&event).await;
        assert!(result.is_ok());
        assert!(!result.unwrap());
        let engine = runtime.engine().read().unwrap();
        let room = engine.rooms().get("living_room").unwrap();
        assert!(room.hard_off);
        assert!(!room.soft_off);
    }

    #[test]
    fn test_set_light_profile_config() {
        let runtime = test_runtime();
        let mut config = crate::default_rhythm_profile();
        config.max_brightness = 90;
        let result = runtime.set_light_profile_config(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_set_light_profile_config_registers_custom_profile() {
        let runtime = test_runtime();
        let mut config = crate::default_rhythm_profile();
        config.id = "day_alt".into();
        config.name = "Day Alt".into();
        config.max_brightness = 77;

        let result = runtime.set_light_profile_config(config.clone());
        assert!(result.is_ok());

        let engine = runtime.engine().read().unwrap();
        let stored = engine.profile_registry().profile_config("day_alt").unwrap();
        assert_eq!(stored.name, "Day Alt");
        assert_eq!(stored.max_brightness, 77);
    }

    #[test]
    fn test_set_solar() {
        let runtime = test_runtime();
        let solar = SolarTime::new(12.5, 35.0, 100);
        let result = runtime.set_solar(solar);
        assert!(result.is_ok());
    }

    #[test]
    fn test_set_sun_times() {
        let runtime = test_runtime();
        let sun_times = SunTimes {
            sunrise: 6.5,
            sunset: 19.5,
            day_length: 13.0,
        };
        let result = runtime.set_sun_times(sun_times);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_sync_rooms() {
        let runtime = test_runtime();
        let result = runtime.sync_rooms().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_current_hour() {
        let runtime = test_runtime();
        let hour = runtime.current_hour();
        // MockTimeProvider defaults to 12.0
        assert!((hour - 12.0).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn test_handle_event_with_room_added() {
        let runtime = test_runtime();

        // Add a room to the engine first
        {
            let mut engine = runtime.engine.write().unwrap();
            engine.rooms_mut().get_or_create("bedroom", "Bedroom");
        }

        let event = InputEvent::new("bedroom", ButtonAction::OnPress);
        let result = runtime.handle_event(&event).await;
        assert!(result.is_ok());
        assert!(result.unwrap()); // OnPress returns true
    }
}
