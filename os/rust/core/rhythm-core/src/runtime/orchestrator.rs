//! RhythmRuntime orchestrator.
//!
//! This module provides the main `RhythmRuntime` struct that ties together
//! the RhythmEngine with scheduling, event handling, and persistence.

use std::sync::{Arc, RwLock};

use tracing::{debug, info};

use crate::controller::LightController;
use crate::light_profile::LightProfileConfig;
use crate::primitives::RhythmEngine;
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
    engine: Arc<RwLock<RhythmEngine<C>>>,

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
        controller: C,
        time_provider: T,
        scheduler: S,
        device_registry: R,
        config: RuntimeConfig,
    ) -> Self {
        let engine = RhythmEngine::new(controller);

        Self {
            engine: Arc::new(RwLock::new(engine)),
            time_provider: Arc::new(time_provider),
            scheduler: Arc::new(scheduler),
            device_registry: Arc::new(RwLock::new(device_registry)),
            config,
        }
    }

    /// Get a reference to the engine (for direct access if needed).
    pub fn engine(&self) -> &Arc<RwLock<RhythmEngine<C>>> {
        &self.engine
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
        if !engine.set_light_profile_config(config) {
            return Err(RuntimeError::ConfigError(
                "Unknown light profile".to_string(),
            ));
        }
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
    #[allow(clippy::await_holding_lock)] // Intentional: sync RwLock with blocking engine ops
    pub async fn handle_event(&self, event: &InputEvent) -> RuntimeResult<bool> {
        let current_hour = self.current_hour();

        debug!(
            "Handling event: room={}, action={:?}, device={:?}",
            event.room_id, event.action, event.device_id
        );

        let mut engine = self
            .engine
            .write()
            .map_err(|e| RuntimeError::Internal(format!("Failed to lock engine: {}", e)))?;

        match event.action {
            ButtonAction::OnPress => {
                info!("turn_on for {}", event.room_id);
                engine.turn_on(&event.room_id, current_hour).await?;
                Ok(true)
            }
            ButtonAction::Toggle => {
                info!("toggle for {}", event.room_id);
                let turned_on = engine.toggle(&event.room_id, current_hour).await?;
                Ok(turned_on)
            }
            ButtonAction::OffPress => {
                info!("turn_off for {}", event.room_id);
                engine.turn_off(&event.room_id, current_hour).await?;
                Ok(false)
            }
            ButtonAction::Reset => {
                info!("reset for {}", event.room_id);
                engine.reset(&event.room_id, current_hour).await?;
                Ok(true)
            }
            ButtonAction::UpPress => {
                info!("dim_up for {}", event.room_id);
                engine.dim_up(&event.room_id, current_hour, None).await?;
                Ok(true)
            }
            ButtonAction::DownPress => {
                info!("dim_down for {}", event.room_id);
                engine.dim_down(&event.room_id, current_hour, None).await?;
                Ok(true)
            }
            ButtonAction::UpHold => {
                info!("step_up for {}", event.room_id);
                engine.step_up(&event.room_id, current_hour).await?;
                Ok(true)
            }
            ButtonAction::DownHold => {
                info!("step_down for {}", event.room_id);
                engine.step_down(&event.room_id, current_hour).await?;
                Ok(true)
            }
            ButtonAction::Stop => {
                debug!("stop (ignored) for {}", event.room_id);
                Ok(false)
            }
            ButtonAction::RhythmOn => {
                info!("rhythm_on for {}", event.room_id);
                engine.rhythm_on(&event.room_id).await?;
                Ok(false) // No lights changed
            }
            ButtonAction::RhythmOff => {
                info!("rhythm_off for {}", event.room_id);
                engine.rhythm_off(&event.room_id).await?;
                Ok(false)
            }
            ButtonAction::LightsOff => {
                info!("lights_off for {}", event.room_id);
                engine.lights_off(&event.room_id).await?;
                Ok(false)
            }
            ButtonAction::SleepOn => {
                info!("sleep_on for {}", event.room_id);
                engine.set_light_profile(crate::light_profile::SLEEP_PROFILE_ID);
                engine.turn_off(&event.room_id, current_hour).await?;
                Ok(false)
            }
            ButtonAction::SleepOff => {
                info!("sleep_off for {}", event.room_id);
                engine.set_light_profile(crate::light_profile::RHYTHM_PROFILE_ID);
                engine.turn_on(&event.room_id, current_hour).await?;
                Ok(true)
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
            NoOpController::new(),
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
