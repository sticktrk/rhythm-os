//! Type-erased runtime handle for `RhythmRuntime<C,T,S,R>`.
//!
//! Provides a trait-object interface so the platform layer (HTTP handlers,
//! command functions, main loop) can interact with any hub's runtime
//! without knowing the concrete generic types.

use crate::controller::LightController;
use crate::light_profile::LightProfileConfig;
use crate::lighting::LightingCommand;
use crate::room::{LightNodeKind, ModeConfig, RoomModeState, RoomProfileSettings};
use crate::solar::SolarTime;
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

    /// Update a light profile configuration.
    fn set_light_profile_config(&self, config: LightProfileConfig) -> Result<()>;

    /// Replace the mode/state profile mappings.
    fn set_mode_configs(&self, configs: Vec<ModeConfig>) -> Result<()>;

    /// Run a periodic update for a single room. Holds the engine lock only
    /// for this one room (~200-500ms) instead of all rooms at once.
    fn periodic_tick_room(&self, room_id: &str, current_hour: f32) -> Result<()>;

    /// Run a periodic update for a derived dispatch node that inherits a
    /// topology room's settings.
    ///
    /// The default implementation falls back to room-level ticking so
    /// non-composite test runtimes keep working.
    fn periodic_tick_node(
        &self,
        _node_id: &str,
        source_room_id: &str,
        current_hour: f32,
    ) -> Result<()> {
        self.periodic_tick_room(source_room_id, current_hour)
    }

    /// Get a read-only snapshot of a single room's state.
    fn engine_room_snapshot(&self, room_id: &str) -> Option<RoomSnapshot>;

    /// Get read-only snapshots of all rooms.
    fn engine_all_room_snapshots(&self) -> Vec<RoomSnapshot>;

    /// Get a read-only snapshot of any addressable runtime node.
    fn engine_node_snapshot(&self, node_id: &str) -> Option<NodeSnapshot> {
        self.engine_room_snapshot(node_id)
            .map(NodeSnapshot::from_room_snapshot)
    }

    /// Get read-only snapshots of all runtime nodes.
    fn engine_all_node_snapshots(&self) -> Vec<NodeSnapshot> {
        self.engine_all_room_snapshots()
            .into_iter()
            .map(NodeSnapshot::from_room_snapshot)
            .collect()
    }

    /// Get an effective inherited snapshot for a runtime node.
    fn engine_effective_node_snapshot(&self, node_id: &str) -> Option<NodeSnapshot> {
        self.engine_node_snapshot(node_id)
    }

    /// Get effective inherited snapshots for all runtime nodes.
    fn engine_all_effective_node_snapshots(&self) -> Vec<NodeSnapshot> {
        self.engine_all_node_snapshots()
    }

    /// Restore persisted room state into the engine.
    fn restore_room_state(&self, room_id: &str, state: RestoredRoomState);

    /// Restore persisted node state into the engine.
    fn restore_node_state(&self, node_id: &str, state: RestoredNodeState) {
        self.restore_room_state(node_id, RestoredRoomState::from(&state));
    }

    /// Add a room to the engine's room manager.
    fn add_room(&self, room_id: &str, room_name: &str);

    /// Add an addressable node to the engine's room manager.
    fn add_node(
        &self,
        node_id: &str,
        node_name: &str,
        kind: LightNodeKind,
        parent_id: Option<String>,
    ) {
        let _ = (kind, parent_id);
        self.add_room(node_id, node_name);
    }

    /// Remove a room from the engine's room manager.
    fn remove_room(&self, room_id: &str);

    /// Remove any addressable node from the engine's room manager.
    fn remove_node(&self, node_id: &str) {
        self.remove_room(node_id);
    }

    /// Dim a room's lights to a fraction of current adaptive brightness.
    /// No-op if room doesn't exist in the engine.
    fn dim_room(&self, room_id: &str, factor: f32) -> Result<()>;

    /// Turn on a room with adaptive lighting. Always sends ON — never toggles.
    /// Used by motion detection to avoid the `any_lights_on` round-trip.
    fn turn_on_room(&self, room_id: &str) -> Result<()>;

    /// Apply an explicit rendered lighting command to a room.
    fn apply_room_command(&self, room_id: &str, command: LightingCommand) -> Result<()>;

    /// Turn a room fully off, optionally fading out first.
    fn lights_off_room(&self, room_id: &str, transition_ms: Option<u32>) -> Result<()>;

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

    /// Get the idle brightness from the active light profile.
    fn idle_brightness(&self) -> u8;

    /// Send a soft-off tick to a room: idle curve color at idle brightness.
    /// Used when `soft_off` preference is toggled on for immediate visual feedback.
    fn soft_off_tick_room(&self, room_id: &str) -> Result<()>;

    /// Query the light controller to check if any lights are on in a room.
    /// Used for initial state sync on startup.
    fn any_lights_on(&self, room_id: &str) -> Result<bool>;

    /// Get the current local hour from the time provider (0.0–24.0).
    fn current_hour(&self) -> f32;

    /// Set the active light profile by ID. Returns true if found.
    fn set_light_profile(&self, id: &str) -> bool;

    /// Get the ID of the currently active light profile.
    fn active_light_profile_id(&self) -> String;

    /// List all registered light profiles as `(id, name)` pairs.
    fn available_light_profiles(&self) -> Vec<(String, String)>;
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
    pub kind: LightNodeKind,
    pub parent_id: Option<String>,
    pub rhythm_enabled: bool,
    pub disabled: bool,
    pub time_offset_minutes: f32,
    pub brightness_offset: f32,
    pub soft_off: bool,
    pub hard_off: bool,
    pub profile_settings: RoomProfileSettings,
}

/// Persisted room state restored into the runtime.
#[derive(Debug, Clone)]
pub struct RestoredRoomState {
    pub rhythm_enabled: bool,
    pub disabled: bool,
    pub time_offset_minutes: f32,
    pub brightness_offset: f32,
    pub soft_off: bool,
    pub hard_off: bool,
    pub profile_settings: RoomProfileSettings,
}

/// Read-only snapshot of any addressable runtime node.
#[derive(Debug, Clone)]
pub struct NodeSnapshot {
    pub id: String,
    pub name: String,
    pub kind: LightNodeKind,
    pub parent_id: Option<String>,
    pub rhythm_enabled: bool,
    pub disabled: bool,
    pub time_offset_minutes: f32,
    pub brightness_offset: f32,
    pub soft_off: bool,
    pub hard_off: bool,
    pub profile_settings: RoomProfileSettings,
}

impl NodeSnapshot {
    pub fn from_room_snapshot(snapshot: RoomSnapshot) -> Self {
        Self {
            id: snapshot.id,
            name: snapshot.name,
            kind: snapshot.kind,
            parent_id: snapshot.parent_id,
            rhythm_enabled: snapshot.rhythm_enabled,
            disabled: snapshot.disabled,
            time_offset_minutes: snapshot.time_offset_minutes,
            brightness_offset: snapshot.brightness_offset,
            soft_off: snapshot.soft_off,
            hard_off: snapshot.hard_off,
            profile_settings: snapshot.profile_settings,
        }
    }
}

/// Persisted node state restored into the runtime.
#[derive(Debug, Clone)]
pub struct RestoredNodeState {
    pub rhythm_enabled: bool,
    pub disabled: bool,
    pub time_offset_minutes: f32,
    pub brightness_offset: f32,
    pub soft_off: bool,
    pub hard_off: bool,
    pub profile_settings: RoomProfileSettings,
}

impl From<&RoomSnapshot> for RestoredRoomState {
    fn from(snapshot: &RoomSnapshot) -> Self {
        Self {
            rhythm_enabled: snapshot.rhythm_enabled,
            disabled: snapshot.disabled,
            time_offset_minutes: snapshot.time_offset_minutes,
            brightness_offset: snapshot.brightness_offset,
            soft_off: snapshot.soft_off,
            hard_off: snapshot.hard_off,
            profile_settings: snapshot.profile_settings.clone(),
        }
    }
}

impl From<&NodeSnapshot> for RestoredNodeState {
    fn from(snapshot: &NodeSnapshot) -> Self {
        Self {
            rhythm_enabled: snapshot.rhythm_enabled,
            disabled: snapshot.disabled,
            time_offset_minutes: snapshot.time_offset_minutes,
            brightness_offset: snapshot.brightness_offset,
            soft_off: snapshot.soft_off,
            hard_off: snapshot.hard_off,
            profile_settings: snapshot.profile_settings.clone(),
        }
    }
}

impl From<&RestoredNodeState> for RestoredRoomState {
    fn from(state: &RestoredNodeState) -> Self {
        Self {
            rhythm_enabled: state.rhythm_enabled,
            disabled: state.disabled,
            time_offset_minutes: state.time_offset_minutes,
            brightness_offset: state.brightness_offset,
            soft_off: state.soft_off,
            hard_off: state.hard_off,
            profile_settings: state.profile_settings.clone(),
        }
    }
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

    fn set_light_profile_config(&self, config: LightProfileConfig) -> Result<()> {
        Ok(RhythmRuntime::set_light_profile_config(self, config)?)
    }

    fn set_mode_configs(&self, configs: Vec<ModeConfig>) -> Result<()> {
        Ok(RhythmRuntime::set_mode_configs(self, configs)?)
    }

    fn periodic_tick_room(&self, room_id: &str, current_hour: f32) -> Result<()> {
        self.periodic_tick_node(room_id, room_id, current_hour)
    }

    fn periodic_tick_node(
        &self,
        node_id: &str,
        source_room_id: &str,
        current_hour: f32,
    ) -> Result<()> {
        let mut engine = self
            .engine()
            .write()
            .map_err(|e| anyhow::anyhow!("Failed to lock engine: {}", e))?;

        let result = crate::runtime::executor::block_on(engine.periodic_tick_node(
            node_id,
            source_room_id,
            current_hour,
        ));

        match result {
            crate::primitives::PeriodicTickResult::Updated => {
                if let Some(room) = engine.rooms().get(source_room_id) {
                    let room_state = RoomModeState::from_flags(room.hard_off, room.soft_off, false);
                    let room_state_label = match room_state {
                        RoomModeState::Active => "active",
                        RoomModeState::Idle => "idle",
                        RoomModeState::Wake => "wake",
                        RoomModeState::Warning => "warning",
                        RoomModeState::HardOff => "hard_off",
                    };
                    let room_label = if room.name != room.id {
                        format!("{} ({})", room.name, room.id)
                    } else {
                        room.id.clone()
                    };
                    let node_label = if node_id == source_room_id {
                        room_label.clone()
                    } else {
                        format!("{room_label} via {node_id}")
                    };
                    let mode = engine.profile_registry().active_mode();
                    let profile_id = engine
                        .profile_registry()
                        .profile_for_room_state(mode, room_state, Some(&room.profile_settings))
                        .id()
                        .to_string();

                    if room_state == RoomModeState::Active {
                        log::debug!(
                            target: "sys",
                            "Periodic tick: room={} state=active profile={}",
                            node_label,
                            profile_id
                        );
                    } else {
                        log::info!(
                            target: "sys",
                            "Periodic tick: room={} state={} profile={}",
                            node_label,
                            room_state_label,
                            profile_id
                        );
                    }
                }
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
            kind: room.kind,
            parent_id: room.parent_id.clone(),
            rhythm_enabled: room.rhythm_enabled,
            disabled: room.disabled,
            time_offset_minutes: room.time_offset_minutes,
            brightness_offset: room.brightness_offset,
            soft_off: room.soft_off,
            hard_off: room.hard_off,
            profile_settings: room.profile_settings.clone(),
        })
    }

    fn engine_all_room_snapshots(&self) -> Vec<RoomSnapshot> {
        let Ok(engine) = self.engine().read() else {
            return Vec::new();
        };
        engine
            .rooms()
            .room_iter()
            .map(|room| RoomSnapshot {
                id: room.id.clone(),
                name: room.name.clone(),
                kind: room.kind,
                parent_id: room.parent_id.clone(),
                rhythm_enabled: room.rhythm_enabled,
                disabled: room.disabled,
                time_offset_minutes: room.time_offset_minutes,
                brightness_offset: room.brightness_offset,
                soft_off: room.soft_off,
                hard_off: room.hard_off,
                profile_settings: room.profile_settings.clone(),
            })
            .collect()
    }

    fn engine_node_snapshot(&self, node_id: &str) -> Option<NodeSnapshot> {
        let engine = self.engine().read().ok()?;
        let node = engine.rooms().get(node_id)?;
        Some(NodeSnapshot {
            id: node.id.clone(),
            name: node.name.clone(),
            kind: node.kind,
            parent_id: node.parent_id.clone(),
            rhythm_enabled: node.rhythm_enabled,
            disabled: node.disabled,
            time_offset_minutes: node.time_offset_minutes,
            brightness_offset: node.brightness_offset,
            soft_off: node.soft_off,
            hard_off: node.hard_off,
            profile_settings: node.profile_settings.clone(),
        })
    }

    fn engine_all_node_snapshots(&self) -> Vec<NodeSnapshot> {
        let Ok(engine) = self.engine().read() else {
            return Vec::new();
        };
        engine
            .rooms()
            .iter()
            .map(|node| NodeSnapshot {
                id: node.id.clone(),
                name: node.name.clone(),
                kind: node.kind,
                parent_id: node.parent_id.clone(),
                rhythm_enabled: node.rhythm_enabled,
                disabled: node.disabled,
                time_offset_minutes: node.time_offset_minutes,
                brightness_offset: node.brightness_offset,
                soft_off: node.soft_off,
                hard_off: node.hard_off,
                profile_settings: node.profile_settings.clone(),
            })
            .collect()
    }

    fn engine_effective_node_snapshot(&self, node_id: &str) -> Option<NodeSnapshot> {
        let engine = self.engine().read().ok()?;
        let node = engine.rooms().get(node_id)?;
        let effective = engine.rooms().effective_state(node_id)?;
        Some(NodeSnapshot {
            id: node.id.clone(),
            name: node.name.clone(),
            kind: node.kind,
            parent_id: node.parent_id.clone(),
            rhythm_enabled: effective.rhythm_enabled,
            disabled: effective.disabled,
            time_offset_minutes: effective.time_offset_minutes,
            brightness_offset: effective.brightness_offset,
            soft_off: effective.soft_off,
            hard_off: effective.hard_off,
            profile_settings: effective.profile_settings,
        })
    }

    fn engine_all_effective_node_snapshots(&self) -> Vec<NodeSnapshot> {
        let Ok(engine) = self.engine().read() else {
            return Vec::new();
        };
        engine
            .rooms()
            .iter()
            .filter_map(|node| {
                let effective = engine.rooms().effective_state(&node.id)?;
                Some(NodeSnapshot {
                    id: node.id.clone(),
                    name: node.name.clone(),
                    kind: node.kind,
                    parent_id: node.parent_id.clone(),
                    rhythm_enabled: effective.rhythm_enabled,
                    disabled: effective.disabled,
                    time_offset_minutes: effective.time_offset_minutes,
                    brightness_offset: effective.brightness_offset,
                    soft_off: effective.soft_off,
                    hard_off: effective.hard_off,
                    profile_settings: effective.profile_settings,
                })
            })
            .collect()
    }

    fn restore_room_state(&self, room_id: &str, state: RestoredRoomState) {
        let has_room_profile = !state.profile_settings.is_empty();
        if let Ok(mut engine) = self.engine().write() {
            let mut restored = false;
            if let Some(room) = engine.rooms_mut().get_mut(room_id) {
                room.rhythm_enabled = state.rhythm_enabled;
                room.disabled = state.disabled;
                room.time_offset_minutes = state.time_offset_minutes;
                room.brightness_offset = state.brightness_offset;
                room.soft_off = state.soft_off;
                room.hard_off = state.hard_off;
                room.profile_settings = state.profile_settings;
                restored = true;
            }
            if restored {
                engine.reset_restored_room_state(room_id);
                log::debug!(
                    target: "sys",
                    "restore_room_state: '{}' rhythm={} disabled={} time_offset={} bri_offset={} soft_off={} hard_off={} room_profile={}",
                    room_id,
                    state.rhythm_enabled,
                    state.disabled,
                    state.time_offset_minutes,
                    state.brightness_offset,
                    state.soft_off,
                    state.hard_off,
                    has_room_profile
                );
            }
        }
    }

    fn restore_node_state(&self, node_id: &str, state: RestoredNodeState) {
        self.restore_room_state(node_id, RestoredRoomState::from(&state));
    }

    fn add_room(&self, room_id: &str, room_name: &str) {
        self.add_node(room_id, room_name, LightNodeKind::Room, None);
    }

    fn add_node(
        &self,
        node_id: &str,
        node_name: &str,
        kind: LightNodeKind,
        parent_id: Option<String>,
    ) {
        if let Ok(mut engine) = self.engine().write() {
            let node =
                engine
                    .rooms_mut()
                    .get_or_create_node(node_id, node_name, kind, parent_id.clone());
            node.name = node_name.to_string();
            node.kind = kind;
            node.parent_id = parent_id;
        }
    }

    fn remove_room(&self, room_id: &str) {
        self.remove_node(room_id);
    }

    fn remove_node(&self, node_id: &str) {
        if let Ok(mut engine) = self.engine().write() {
            engine.remove_room_state(node_id);
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

    fn apply_room_command(&self, room_id: &str, command: LightingCommand) -> Result<()> {
        log::debug!(
            target: "cmd",
            "apply_room_command: room='{}' bri={} kelvin={} transition_ms={:?} direct_color={}",
            room_id,
            command.brightness,
            command.kelvin,
            command.transition_ms,
            command.is_direct_color
        );
        let mut engine = self
            .engine()
            .write()
            .map_err(|e| anyhow::anyhow!("Failed to lock engine: {}", e))?;
        crate::runtime::executor::block_on(engine.apply_room_command(room_id, command))
            .map_err(|e| anyhow::anyhow!("apply_room_command failed: {}", e))
    }

    fn lights_off_room(&self, room_id: &str, transition_ms: Option<u32>) -> Result<()> {
        let mut engine = self
            .engine()
            .write()
            .map_err(|e| anyhow::anyhow!("Failed to lock engine: {}", e))?;
        crate::runtime::executor::block_on(engine.lights_off(room_id, transition_ms))
            .map_err(|e| anyhow::anyhow!("lights_off failed: {}", e))
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

    fn idle_brightness(&self) -> u8 {
        let hour = self.current_hour();
        self.engine()
            .read()
            .map(|e| e.idle_brightness(hour))
            .unwrap_or(1)
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

    fn set_light_profile(&self, id: &str) -> bool {
        if let Ok(mut engine) = self.engine().write() {
            engine.set_light_profile(id)
        } else {
            false
        }
    }

    fn active_light_profile_id(&self) -> String {
        self.engine()
            .read()
            .map(|e| e.profile_registry().active_profile_id().to_string())
            .unwrap_or_default()
    }

    fn available_light_profiles(&self) -> Vec<(String, String)> {
        self.engine()
            .read()
            .map(|e| {
                e.profile_registry()
                    .available_profiles()
                    .into_iter()
                    .map(|(id, name)| (id.to_string(), name.to_string()))
                    .collect()
            })
            .unwrap_or_default()
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

        handle.restore_room_state(
            "living",
            RestoredRoomState {
                rhythm_enabled: false,
                disabled: true,
                time_offset_minutes: 30.0,
                brightness_offset: 5.0,
                soft_off: true,
                hard_off: false,
                profile_settings: crate::RoomProfileSettings::default(),
            },
        );

        let snap = handle.engine_room_snapshot("living").unwrap();
        assert!(!snap.rhythm_enabled);
        assert!(snap.disabled);
        assert!((snap.time_offset_minutes - 30.0).abs() < f32::EPSILON);
        assert!((snap.brightness_offset - 5.0).abs() < f32::EPSILON);
        assert!(snap.soft_off);
        assert!(!snap.hard_off);
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
