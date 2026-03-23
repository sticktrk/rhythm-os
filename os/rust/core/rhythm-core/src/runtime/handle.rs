//! Type-erased runtime handle for `RhythmRuntime<C,T,S,R>`.
//!
//! Provides a trait-object interface so the platform layer (HTTP handlers,
//! command functions, main loop) can interact with any hub's runtime
//! without knowing the concrete generic types.

use crate::controller::LightController;
use crate::solar::SolarTime;
use crate::CurveConfig;
use anyhow::Result;

use crate::runtime::events::InputEvent;
use crate::runtime::{DeviceRegistry, RhythmRuntime, Scheduler, TimeProvider};

// ============================================================================
// RuntimeHandle — type-erased interface to RhythmRuntime<C,T,S,R>
// ============================================================================

/// Type-erased interface to a `RhythmRuntime<C,T,S,R>`.
///
/// This trait allows the platform layer to interact with any hub's runtime
/// without knowing the concrete generic types. The blanket implementation
/// on `RhythmRuntime` means any hub automatically satisfies this trait
/// with zero boilerplate.
pub trait RuntimeHandle: Send + Sync {
    /// Route a button/service event through the engine.
    /// Returns true if lights were turned on.
    fn handle_event(&self, event: &InputEvent) -> Result<bool>;

    /// Sync rooms from the light controller.
    fn sync_rooms(&self) -> Result<()>;

    /// Update the solar time parameters.
    fn set_solar(&self, solar: SolarTime) -> Result<()>;

    /// Update the curve configuration.
    fn set_curve_config(&self, config: CurveConfig) -> Result<()>;

    /// Run a periodic update for a single room. Holds the engine lock only
    /// for this one room (~200-500ms) instead of all rooms at once.
    fn periodic_tick_room(&self, room_id: &str, current_hour: f32) -> Result<()>;

    /// Get a read-only snapshot of a single room's state.
    fn engine_room_snapshot(&self, room_id: &str) -> Option<RoomSnapshot>;

    /// Get read-only snapshots of all rooms.
    fn engine_all_room_snapshots(&self) -> Vec<RoomSnapshot>;

    /// Restore persisted room state into the engine.
    fn restore_room_state(
        &self,
        room_id: &str,
        rhythm_enabled: bool,
        disabled: bool,
        time_offset: f32,
        bri_offset: f32,
        soft_off: bool,
    );

    /// Add a room to the engine's room manager.
    fn add_room(&self, room_id: &str, room_name: &str);

    /// Remove a room from the engine's room manager.
    fn remove_room(&self, room_id: &str);

    /// Dim a room's lights to a fraction of current adaptive brightness.
    /// No-op if room doesn't exist in the engine.
    fn dim_room(&self, room_id: &str, factor: f32) -> Result<()>;

    /// Turn on a room with adaptive lighting. Always sends ON — never toggles.
    /// Used by motion detection to avoid the `any_lights_on` round-trip.
    fn turn_on_room(&self, room_id: &str) -> Result<()>;

    /// Set power save mode on the engine. Returns room IDs that were soft_off
    /// (caller must turn them truly off when switching power_save ON).
    fn set_power_save(&self, enabled: bool) -> Vec<String>;

    /// Get whether power save mode is enabled.
    fn is_power_save(&self) -> bool;

    /// Set absolute brightness for a room (1-100).
    /// Computes the offset so effective brightness equals the target.
    fn set_room_brightness(&self, room_id: &str, brightness: u8) -> Result<()>;

    /// Set the time offset for a room directly (not additive).
    fn set_room_time_offset(&self, room_id: &str, offset_minutes: f32) -> Result<()>;

    /// Set the soft-off brightness percentage on the engine.
    fn set_soft_off_brightness(&self, value: u8);

    /// Send a soft-off tick to a room: adaptive color temp at soft-off brightness.
    /// Used when `soft_off` preference is toggled on for immediate visual feedback.
    fn soft_off_tick_room(&self, room_id: &str) -> Result<()>;

    /// Query the light controller to check if any lights are on in a room.
    /// Used for initial state sync on startup.
    fn any_lights_on(&self, room_id: &str) -> Result<bool>;

    /// Get the current local hour from the time provider (0.0–24.0).
    fn current_hour(&self) -> f32;
}

// ============================================================================
// RoomSnapshot — read-only room state for HTTP responses
// ============================================================================

/// Read-only snapshot of a room's state from the engine.
///
/// Avoids leaking engine generics across the hub boundary.
#[derive(Debug, Clone)]
pub struct RoomSnapshot {
    pub id: String,
    pub name: String,
    pub rhythm_enabled: bool,
    pub disabled: bool,
    pub time_offset_minutes: f32,
    pub brightness_offset: f32,
    pub soft_off: bool,
}

// ============================================================================
// Blanket impl — any RhythmRuntime<C,T,S,R> satisfies RuntimeHandle
// ============================================================================

impl<C, T, S, R> RuntimeHandle for RhythmRuntime<C, T, S, R>
where
    C: LightController + Send + Sync + 'static,
    T: TimeProvider + Send + Sync + 'static,
    S: Scheduler + Send + Sync + 'static,
    R: DeviceRegistry + Send + Sync + 'static,
{
    fn handle_event(&self, event: &InputEvent) -> Result<bool> {
        // Inherent async method takes priority over trait method in resolution.
        Ok(crate::runtime::executor::block_on(
            self.handle_event(event),
        )?)
    }

    fn sync_rooms(&self) -> Result<()> {
        Ok(crate::runtime::executor::block_on(self.sync_rooms())?)
    }

    fn set_solar(&self, solar: SolarTime) -> Result<()> {
        // Inherent sync method — same name is fine, inherent takes priority.
        Ok(RhythmRuntime::set_solar(self, solar)?)
    }

    fn set_curve_config(&self, config: CurveConfig) -> Result<()> {
        Ok(RhythmRuntime::set_curve_config(self, config)?)
    }

    fn periodic_tick_room(&self, room_id: &str, current_hour: f32) -> Result<()> {
        let mut engine = self
            .engine()
            .write()
            .map_err(|e| anyhow::anyhow!("Failed to lock engine: {}", e))?;

        let result = crate::runtime::executor::block_on(
            engine.periodic_tick_single_room(room_id, current_hour),
        );

        match result {
            crate::primitives::PeriodicTickResult::Updated => {
                log::info!(target: "sys", "Periodic room tick: '{}' updated", room_id);
            }
            crate::primitives::PeriodicTickResult::Error(e) => {
                log::warn!(target: "sys", "Periodic room tick failed: {}", e);
            }
            crate::primitives::PeriodicTickResult::Skipped => {}
        }
        Ok(())
    }

    fn engine_room_snapshot(&self, room_id: &str) -> Option<RoomSnapshot> {
        let engine = self.engine().read().ok()?;
        let room = engine.rooms().get(room_id)?;
        Some(RoomSnapshot {
            id: room.id.clone(),
            name: room.name.clone(),
            rhythm_enabled: room.rhythm_enabled,
            disabled: room.disabled,
            time_offset_minutes: room.time_offset_minutes,
            brightness_offset: room.brightness_offset,
            soft_off: room.soft_off,
        })
    }

    fn engine_all_room_snapshots(&self) -> Vec<RoomSnapshot> {
        let Ok(engine) = self.engine().read() else {
            return Vec::new();
        };
        engine
            .rooms()
            .iter()
            .map(|room| RoomSnapshot {
                id: room.id.clone(),
                name: room.name.clone(),
                rhythm_enabled: room.rhythm_enabled,
                disabled: room.disabled,
                time_offset_minutes: room.time_offset_minutes,
                brightness_offset: room.brightness_offset,
                soft_off: room.soft_off,
            })
            .collect()
    }

    fn restore_room_state(
        &self,
        room_id: &str,
        rhythm_enabled: bool,
        disabled: bool,
        time_offset: f32,
        bri_offset: f32,
        soft_off: bool,
    ) {
        if let Ok(mut engine) = self.engine().write() {
            if let Some(room) = engine.rooms_mut().get_mut(room_id) {
                room.rhythm_enabled = rhythm_enabled;
                room.disabled = disabled;
                room.time_offset_minutes = time_offset;
                room.brightness_offset = bri_offset;
                room.soft_off = soft_off;
            }
        }
    }

    fn add_room(&self, room_id: &str, room_name: &str) {
        if let Ok(mut engine) = self.engine().write() {
            engine.rooms_mut().get_or_create(room_id, room_name);
        }
    }

    fn remove_room(&self, room_id: &str) {
        if let Ok(mut engine) = self.engine().write() {
            engine.rooms_mut().remove(room_id);
        }
    }

    fn dim_room(&self, room_id: &str, factor: f32) -> Result<()> {
        let current_hour = self.current_hour();
        let mut engine = self
            .engine()
            .write()
            .map_err(|e| anyhow::anyhow!("Failed to lock engine: {}", e))?;
        crate::runtime::executor::block_on(engine.dim_to_factor(room_id, current_hour, factor))
            .map_err(|e| anyhow::anyhow!("dim_to_factor failed: {}", e))
    }

    fn turn_on_room(&self, room_id: &str) -> Result<()> {
        let current_hour = self.current_hour();
        let mut engine = self
            .engine()
            .write()
            .map_err(|e| anyhow::anyhow!("Failed to lock engine: {}", e))?;
        crate::runtime::executor::block_on(engine.turn_on(room_id, current_hour))
            .map_err(|e| anyhow::anyhow!("turn_on failed: {}", e))
    }

    fn set_power_save(&self, enabled: bool) -> Vec<String> {
        if let Ok(mut engine) = self.engine().write() {
            engine.set_power_save(enabled)
        } else {
            Vec::new()
        }
    }

    fn is_power_save(&self) -> bool {
        self.engine().read().map(|e| e.power_save()).unwrap_or(true)
    }

    fn set_room_brightness(&self, room_id: &str, brightness: u8) -> Result<()> {
        let current_hour = self.current_hour();
        let mut engine = self
            .engine()
            .write()
            .map_err(|e| anyhow::anyhow!("Failed to lock engine: {}", e))?;
        crate::runtime::executor::block_on(engine.set_brightness(room_id, current_hour, brightness))
            .map_err(|e| anyhow::anyhow!("set_brightness failed: {}", e))
    }

    fn set_room_time_offset(&self, room_id: &str, offset_minutes: f32) -> Result<()> {
        let current_hour = self.current_hour();
        let mut engine = self
            .engine()
            .write()
            .map_err(|e| anyhow::anyhow!("Failed to lock engine: {}", e))?;
        crate::runtime::executor::block_on(engine.set_time_offset(
            room_id,
            current_hour,
            offset_minutes,
        ))
        .map_err(|e| anyhow::anyhow!("set_time_offset failed: {}", e))
    }

    fn set_soft_off_brightness(&self, value: u8) {
        if let Ok(mut engine) = self.engine().write() {
            engine.set_soft_off_brightness(value);
        }
    }

    fn soft_off_tick_room(&self, room_id: &str) -> Result<()> {
        let current_hour = self.current_hour();
        let mut engine = self
            .engine()
            .write()
            .map_err(|e| anyhow::anyhow!("Failed to lock engine: {}", e))?;
        crate::runtime::executor::block_on(engine.soft_off_tick(room_id, current_hour))
            .map_err(|e| anyhow::anyhow!("soft_off_tick failed: {}", e))
    }

    fn any_lights_on(&self, room_id: &str) -> Result<bool> {
        let engine = self
            .engine()
            .read()
            .map_err(|e| anyhow::anyhow!("Failed to lock engine: {}", e))?;
        crate::runtime::executor::block_on(engine.controller().any_lights_on(room_id))
            .map_err(|e| anyhow::anyhow!("any_lights_on failed: {}", e))
    }

    fn current_hour(&self) -> f32 {
        RhythmRuntime::current_hour(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::NoOpController;
    use crate::runtime::config::RuntimeConfig;
    use crate::runtime::orchestrator::RhythmRuntime;
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

    fn as_handle(
        rt: &RhythmRuntime<NoOpController, MockTimeProvider, NoOpScheduler, SimpleDeviceRegistry>,
    ) -> &dyn RuntimeHandle {
        rt
    }

    #[test]
    fn engine_room_snapshot_missing_returns_none() {
        let rt = test_runtime();
        let handle = as_handle(&rt);
        assert!(handle.engine_room_snapshot("nonexistent").is_none());
    }

    #[test]
    fn engine_room_snapshot_returns_some_for_existing() {
        let rt = test_runtime();
        let handle = as_handle(&rt);
        handle.add_room("kitchen", "Kitchen");
        let snap = handle.engine_room_snapshot("kitchen");
        assert!(snap.is_some());
        let snap = snap.unwrap();
        assert_eq!(snap.id, "kitchen");
        assert_eq!(snap.name, "Kitchen");
    }

    #[test]
    fn engine_all_room_snapshots_empty_initially() {
        let rt = test_runtime();
        let handle = as_handle(&rt);
        assert!(handle.engine_all_room_snapshots().is_empty());
    }

    #[test]
    fn engine_all_room_snapshots_populated_after_add() {
        let rt = test_runtime();
        let handle = as_handle(&rt);
        handle.add_room("room1", "Room 1");
        handle.add_room("room2", "Room 2");
        let snaps = handle.engine_all_room_snapshots();
        assert_eq!(snaps.len(), 2);
    }

    #[test]
    fn add_and_remove_room() {
        let rt = test_runtime();
        let handle = as_handle(&rt);
        handle.add_room("temp", "Temp Room");
        assert!(handle.engine_room_snapshot("temp").is_some());
        handle.remove_room("temp");
        assert!(handle.engine_room_snapshot("temp").is_none());
    }

    #[test]
    fn restore_room_state_updates_snapshot() {
        let rt = test_runtime();
        let handle = as_handle(&rt);
        handle.add_room("living", "Living Room");

        handle.restore_room_state("living", false, true, 30.0, 5.0, true);

        let snap = handle.engine_room_snapshot("living").unwrap();
        assert!(!snap.rhythm_enabled);
        assert!(snap.disabled);
        assert!((snap.time_offset_minutes - 30.0).abs() < f32::EPSILON);
        assert!((snap.brightness_offset - 5.0).abs() < f32::EPSILON);
        assert!(snap.soft_off);
    }

    // Tests that call block_on() internally require the tokio feature flag.
    // They run under `cargo test -p rhythm-core --features tokio` or
    // via `cargo test` (workspace enables tokio through rhythm-os/rhythm-addon).

    #[cfg(feature = "tokio")]
    #[test]
    fn periodic_tick_room_succeeds() {
        let rt = test_runtime();
        let handle = as_handle(&rt);
        handle.add_room("room1", "Room 1");
        let result = handle.periodic_tick_room("room1", 14.0);
        assert!(result.is_ok());
    }

    #[cfg(feature = "tokio")]
    #[test]
    fn set_room_brightness_succeeds() {
        let rt = test_runtime();
        let handle = as_handle(&rt);
        handle.add_room("room1", "Room 1");
        let result = handle.set_room_brightness("room1", 75);
        assert!(result.is_ok());
    }

    #[cfg(feature = "tokio")]
    #[test]
    fn set_room_time_offset_succeeds() {
        let rt = test_runtime();
        let handle = as_handle(&rt);
        handle.add_room("room1", "Room 1");
        let result = handle.set_room_time_offset("room1", 30.0);
        assert!(result.is_ok());
    }

    #[test]
    fn set_and_query_power_save() {
        let rt = test_runtime();
        let handle = as_handle(&rt);
        // Default is false (DEFAULT_POWER_SAVE = false)
        assert!(!handle.is_power_save());
        handle.set_power_save(false);
        assert!(!handle.is_power_save());
        handle.set_power_save(true);
        assert!(handle.is_power_save());
    }

    #[test]
    fn set_soft_off_brightness() {
        let rt = test_runtime();
        let handle = as_handle(&rt);
        // Should not panic
        handle.set_soft_off_brightness(25);
    }

    #[cfg(feature = "tokio")]
    #[test]
    fn any_lights_on_returns_false_for_noop() {
        let rt = test_runtime();
        let handle = as_handle(&rt);
        // NoOpController always returns false
        let result = handle.any_lights_on("room1");
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }
}
