//! Type-erased runtime handle for `RhythmRuntime<C,T,S,R>`.
//!
//! Provides a trait-object interface so the platform layer (HTTP handlers,
//! command functions, main loop) can interact with any hub's runtime
//! without knowing the concrete generic types.

use crate::controller::LightController;
use crate::light_profile::LightProfileConfig;
use crate::lighting::LightingCommand;
use crate::primitives::{ManualDispatchPlan, PeriodicTickPlan};
use crate::room::{LightNodeKind, ModeConfig, RoomModeState, RoomProfileSettings};
use crate::solar::{SolarTime, SunTimes};
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

    /// Update the current day's sunrise/sunset times.
    fn set_sun_times(&self, _sun_times: SunTimes) -> Result<()> {
        Ok(())
    }

    /// Clear sunrise/sunset times so fallback values are used.
    fn clear_sun_times(&self) -> Result<()> {
        Ok(())
    }

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

    /// Set power save mode on the engine. Returns soft-off room IDs converted
    /// to hard-off and needing a physical off refresh.
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

fn dispatch_manual_plan<C, T, S, R>(
    runtime: &RhythmRuntime<C, T, S, R>,
    dispatch: ManualDispatchPlan,
) -> Result<()>
where
    C: LightController + Send + Sync + 'static,
    T: TimeProvider + Send + Sync + 'static,
    S: Scheduler + Send + Sync + 'static,
    R: DeviceRegistry + Send + Sync + 'static,
{
    match dispatch {
        ManualDispatchPlan::TurnOn {
            source_room_id,
            target_id,
            command,
        } => {
            crate::runtime::executor::block_on(
                runtime.controller().turn_on(&target_id, command.clone()),
            )
            .map_err(|e| anyhow::anyhow!("turn_on failed: {}", e))?;

            let mut engine = runtime
                .engine()
                .write()
                .map_err(|e| anyhow::anyhow!("Failed to lock engine: {}", e))?;
            engine.record_turn_on_dispatch(&source_room_id, &target_id, command);
            Ok(())
        }
        ManualDispatchPlan::TurnOff {
            target_id,
            transition_ms,
        } => crate::runtime::executor::block_on(
            runtime.controller().turn_off(&target_id, transition_ms),
        )
        .map_err(|e| anyhow::anyhow!("turn_off failed: {}", e)),
    }
}

impl<C, T, S, R> RuntimeHandle for RhythmRuntime<C, T, S, R>
where
    C: LightController + Send + Sync + 'static,
    T: TimeProvider + Send + Sync + 'static,
    S: Scheduler + Send + Sync + 'static,
    R: DeviceRegistry + Send + Sync + 'static,
{
    fn handle_event(&self, event: &InputEvent) -> Result<bool> {
        Ok(crate::runtime::executor::block_on(
            RhythmRuntime::handle_event(self, event),
        )?)
    }

    fn sync_rooms(&self) -> Result<()> {
        Ok(crate::runtime::executor::block_on(
            RhythmRuntime::sync_rooms(self),
        )?)
    }

    fn set_solar(&self, solar: SolarTime) -> Result<()> {
        // Inherent sync method — same name is fine, inherent takes priority.
        Ok(RhythmRuntime::set_solar(self, solar)?)
    }

    fn set_sun_times(&self, sun_times: SunTimes) -> Result<()> {
        Ok(RhythmRuntime::set_sun_times(self, sun_times)?)
    }

    fn clear_sun_times(&self) -> Result<()> {
        Ok(RhythmRuntime::clear_sun_times(self)?)
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
        let dispatch_lock = self.dispatch_lock(node_id);
        let _dispatch_guard = dispatch_lock
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock dispatch gate: {}", e))?;
        let controller = self.controller();
        let mut lights_on = None;

        loop {
            let (node_label, plan) = {
                let mut engine = self
                    .engine()
                    .write()
                    .map_err(|e| anyhow::anyhow!("Failed to lock engine: {}", e))?;

                let room_label = engine
                    .rooms()
                    .get(source_room_id)
                    .map(|room| {
                        crate::composite_controller::format_node_log_label(
                            source_room_id,
                            Some(&room.name),
                        )
                    })
                    .unwrap_or_else(|| source_room_id.to_string());
                let node_label = if node_id == source_room_id {
                    room_label.clone()
                } else if let Some(node) = engine.rooms().get(node_id) {
                    crate::composite_controller::format_node_log_label(node_id, Some(&node.name))
                } else {
                    format!(
                        "{} via {}",
                        room_label,
                        crate::composite_controller::format_node_log_label(node_id, None)
                    )
                };

                (
                    node_label,
                    engine.plan_periodic_tick_node(
                        node_id,
                        source_room_id,
                        current_hour,
                        lights_on,
                    ),
                )
            };

            match plan {
                PeriodicTickPlan::Skipped => return Ok(()),
                PeriodicTickPlan::RequiresLightCheck => {
                    lights_on =
                        match crate::runtime::executor::block_on(controller.any_lights_on(node_id))
                        {
                            Ok(is_on) => Some(is_on),
                            Err(_) => return Ok(()),
                        };
                }
                PeriodicTickPlan::Dispatch {
                    command,
                    room_state,
                    profile_id,
                } => {
                    match crate::runtime::executor::block_on(
                        controller.turn_on(node_id, command.clone()),
                    ) {
                        Ok(()) => {
                            let mut engine = self
                                .engine()
                                .write()
                                .map_err(|e| anyhow::anyhow!("Failed to lock engine: {}", e))?;
                            engine.record_turn_on_dispatch(source_room_id, node_id, command);

                            let room_state_label = match room_state {
                                RoomModeState::Active => "active",
                                RoomModeState::Idle => "idle",
                                RoomModeState::Wake => "wake",
                                RoomModeState::Warning => "warning",
                                RoomModeState::HardOff => "hard_off",
                            };

                            tracing::debug!(
                                target: "sys",
                                event = "periodic_room_tick",
                                room = %node_label,
                                state = %room_state_label,
                                profile_id = %profile_id,
                                "Periodic room tick"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                target: "sys",
                                event = "periodic_room_tick_failed",
                                room = %node_label,
                                error = %e,
                                "Periodic room tick failed"
                            );
                        }
                    }
                    return Ok(());
                }
            }
        }
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
        let dispatch_lock = self.dispatch_lock(room_id);
        let _dispatch_guard = dispatch_lock
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock dispatch gate: {}", e))?;
        let current_hour = self.current_hour();
        let dispatch = {
            let mut engine = self
                .engine()
                .write()
                .map_err(|e| anyhow::anyhow!("Failed to lock engine: {}", e))?;
            engine.plan_dim_to_factor(room_id, current_hour, factor)
        };
        if let Some(dispatch) = dispatch {
            dispatch_manual_plan(self, dispatch)
                .map_err(|e| anyhow::anyhow!("dim_to_factor failed: {}", e))
        } else {
            Ok(())
        }
    }

    fn turn_on_room(&self, room_id: &str) -> Result<()> {
        let dispatch_lock = self.dispatch_lock(room_id);
        let _dispatch_guard = dispatch_lock
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock dispatch gate: {}", e))?;
        let current_hour = self.current_hour();
        let dispatch = {
            let mut engine = self
                .engine()
                .write()
                .map_err(|e| anyhow::anyhow!("Failed to lock engine: {}", e))?;
            engine.plan_turn_on(room_id, current_hour)
        };
        dispatch_manual_plan(self, dispatch)
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
        let dispatch_lock = self.dispatch_lock(room_id);
        let _dispatch_guard = dispatch_lock
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock dispatch gate: {}", e))?;
        {
            let mut engine = self
                .engine()
                .write()
                .map_err(|e| anyhow::anyhow!("Failed to lock engine: {}", e))?;
            engine.invalidate_periodic_cache_for_room(room_id);
        }

        dispatch_manual_plan(
            self,
            ManualDispatchPlan::TurnOn {
                source_room_id: room_id.to_string(),
                target_id: room_id.to_string(),
                command,
            },
        )
        .map_err(|e| anyhow::anyhow!("apply_room_command failed: {}", e))
    }

    fn lights_off_room(&self, room_id: &str, transition_ms: Option<u32>) -> Result<()> {
        let dispatch_lock = self.dispatch_lock(room_id);
        let _dispatch_guard = dispatch_lock
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock dispatch gate: {}", e))?;
        let dispatch = {
            let mut engine = self
                .engine()
                .write()
                .map_err(|e| anyhow::anyhow!("Failed to lock engine: {}", e))?;
            engine.plan_lights_off(room_id, transition_ms)
        };
        dispatch_manual_plan(self, dispatch)
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
        let dispatch_lock = self.dispatch_lock(room_id);
        let _dispatch_guard = dispatch_lock
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock dispatch gate: {}", e))?;
        let current_hour = self.current_hour();
        let dispatch = {
            let mut engine = self
                .engine()
                .write()
                .map_err(|e| anyhow::anyhow!("Failed to lock engine: {}", e))?;
            engine.plan_set_brightness(room_id, current_hour, brightness)
        };
        dispatch_manual_plan(self, dispatch)
            .map_err(|e| anyhow::anyhow!("set_brightness failed: {}", e))
    }

    fn set_room_time_offset(&self, room_id: &str, offset_minutes: f32) -> Result<()> {
        let dispatch_lock = self.dispatch_lock(room_id);
        let _dispatch_guard = dispatch_lock
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock dispatch gate: {}", e))?;
        let current_hour = self.current_hour();
        let dispatch = {
            let mut engine = self
                .engine()
                .write()
                .map_err(|e| anyhow::anyhow!("Failed to lock engine: {}", e))?;
            engine.plan_set_time_offset(room_id, current_hour, offset_minutes)
        };
        if let Some(dispatch) = dispatch {
            dispatch_manual_plan(self, dispatch)
                .map_err(|e| anyhow::anyhow!("set_time_offset failed: {}", e))
        } else {
            Ok(())
        }
    }

    fn idle_brightness(&self) -> u8 {
        let hour = self.current_hour();
        self.engine()
            .read()
            .map(|e| e.idle_brightness(hour))
            .unwrap_or(1)
    }

    fn soft_off_tick_room(&self, room_id: &str) -> Result<()> {
        let dispatch_lock = self.dispatch_lock(room_id);
        let _dispatch_guard = dispatch_lock
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock dispatch gate: {}", e))?;
        let current_hour = self.current_hour();
        let dispatch = {
            let mut engine = self
                .engine()
                .write()
                .map_err(|e| anyhow::anyhow!("Failed to lock engine: {}", e))?;
            engine.plan_soft_off_tick(room_id, current_hour)
        };
        dispatch_manual_plan(self, dispatch)
            .map_err(|e| anyhow::anyhow!("soft_off_tick failed: {}", e))
    }

    fn any_lights_on(&self, room_id: &str) -> Result<bool> {
        crate::runtime::executor::block_on(self.controller().any_lights_on(room_id))
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
    use std::sync::Arc;

    use crate::controller::NoOpController;
    use crate::runtime::config::RuntimeConfig;
    use crate::runtime::orchestrator::RhythmRuntime;
    use crate::runtime::registry::SimpleDeviceRegistry;
    use crate::runtime::scheduler::NoOpScheduler;
    use crate::runtime::time::MockTimeProvider;
    use crate::spy_controller::{SpyCall, SpyLightController};
    use std::time::{Duration, Instant};

    type SpyTestRuntime =
        RhythmRuntime<SpyLightController, MockTimeProvider, NoOpScheduler, SimpleDeviceRegistry>;
    type SharedSpyTestRuntime = Arc<SpyTestRuntime>;

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

    fn spy_runtime() -> (SharedSpyTestRuntime, Arc<SpyLightController>) {
        let spy = Arc::new(SpyLightController::new());
        let runtime = Arc::new(RhythmRuntime::new(
            spy.clone(),
            MockTimeProvider::default(),
            NoOpScheduler::new(),
            SimpleDeviceRegistry::new(),
            RuntimeConfig::default(),
        ));
        (runtime, spy)
    }

    fn restored_active_room_state() -> RestoredRoomState {
        RestoredRoomState {
            rhythm_enabled: true,
            disabled: false,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
            soft_off: false,
            hard_off: false,
            profile_settings: crate::RoomProfileSettings::default(),
        }
    }

    fn wait_for_any_lights_on_call(spy: &SpyLightController, room_id: &str) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if spy.calls().into_iter().any(|call| {
                matches!(call, SpyCall::AnyLightsOn { room_id: call_room_id } if call_room_id == room_id)
            }) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("Timed out waiting for any_lights_on({})", room_id);
    }

    // Tests that call block_on() internally require a sync executor feature.
    // The workspace normally exercises these through rhythm-os/rhythm-addon.

    #[test]
    fn periodic_tick_room_succeeds() {
        let rt = test_runtime();
        let handle = as_handle(&rt);
        handle.add_room("room1", "Room 1");
        let result = handle.periodic_tick_room("room1", 14.0);
        assert!(result.is_ok());
    }

    #[test]
    fn set_room_brightness_succeeds() {
        let rt = test_runtime();
        let handle = as_handle(&rt);
        handle.add_room("room1", "Room 1");
        let result = handle.set_room_brightness("room1", 75);
        assert!(result.is_ok());
    }

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
        // Default is true (DEFAULT_POWER_SAVE = true)
        assert!(handle.is_power_save());
        handle.set_power_save(false);
        assert!(!handle.is_power_save());
        handle.set_power_save(true);
        assert!(handle.is_power_save());
    }

    #[test]
    fn any_lights_on_returns_false_for_noop() {
        let rt = test_runtime();
        let handle = as_handle(&rt);
        // NoOpController always returns false
        let result = handle.any_lights_on("room1");
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    #[test]
    fn slow_periodic_tick_does_not_block_turn_on_room_for_other_room() {
        let (runtime, spy) = spy_runtime();
        runtime.add_room("room-a", "Room A");
        runtime.add_room("room-b", "Room B");
        runtime.restore_room_state("room-a", restored_active_room_state());
        spy.set_any_lights_on(true);
        spy.set_turn_on_delay_for_room("room-a", Duration::from_millis(250));

        let runtime_for_thread = runtime.clone();
        let background = std::thread::spawn(move || {
            RuntimeHandle::periodic_tick_node(runtime_for_thread.as_ref(), "room-a", "room-a", 14.0)
        });

        wait_for_any_lights_on_call(&spy, "room-a");

        let start = Instant::now();
        RuntimeHandle::turn_on_room(runtime.as_ref(), "room-b").unwrap();
        let elapsed = start.elapsed();

        background.join().unwrap().unwrap();

        assert!(
            elapsed < Duration::from_millis(150),
            "turn_on_room on room-b took {:?} while room-a periodic dispatch was slow",
            elapsed
        );
        let turn_on_rooms: Vec<_> = spy
            .turn_on_calls()
            .into_iter()
            .map(|(room_id, _)| room_id)
            .collect();
        assert!(turn_on_rooms.contains(&"room-a".to_string()));
        assert!(turn_on_rooms.contains(&"room-b".to_string()));
    }

    #[test]
    fn slow_periodic_tick_does_not_block_button_press_for_other_room() {
        let (runtime, spy) = spy_runtime();
        runtime.add_room("room-a", "Room A");
        runtime.add_room("room-b", "Room B");
        runtime.restore_room_state("room-a", restored_active_room_state());
        spy.set_any_lights_on(true);
        spy.set_turn_on_delay_for_room("room-a", Duration::from_millis(250));

        let runtime_for_thread = runtime.clone();
        let background = std::thread::spawn(move || {
            RuntimeHandle::periodic_tick_node(runtime_for_thread.as_ref(), "room-a", "room-a", 14.0)
        });

        wait_for_any_lights_on_call(&spy, "room-a");

        let start = Instant::now();
        RuntimeHandle::handle_event(
            runtime.as_ref(),
            &InputEvent::new("room-b", crate::runtime::events::ButtonAction::OnPress),
        )
        .unwrap();
        let elapsed = start.elapsed();

        background.join().unwrap().unwrap();

        assert!(
            elapsed < Duration::from_millis(150),
            "handle_event on room-b took {:?} while room-a periodic dispatch was slow",
            elapsed
        );
        let turn_on_rooms: Vec<_> = spy
            .turn_on_calls()
            .into_iter()
            .map(|(room_id, _)| room_id)
            .collect();
        assert!(turn_on_rooms.contains(&"room-a".to_string()));
        assert!(turn_on_rooms.contains(&"room-b".to_string()));
    }

    #[test]
    fn apply_room_command_preserves_periodic_dedupe_cache() {
        let (runtime, spy) = spy_runtime();
        runtime.add_room("room-a", "Room A");
        runtime.restore_room_state("room-a", restored_active_room_state());
        spy.set_any_lights_on(true);

        RuntimeHandle::periodic_tick_node(runtime.as_ref(), "room-a", "room-a", 14.0).unwrap();
        let command = spy
            .last_command_for("room-a")
            .expect("periodic tick should dispatch command");

        spy.reset();
        RuntimeHandle::apply_room_command(runtime.as_ref(), "room-a", command).unwrap();
        assert_eq!(spy.turn_on_count(), 1);

        spy.reset();
        RuntimeHandle::periodic_tick_node(runtime.as_ref(), "room-a", "room-a", 14.0).unwrap();
        assert_eq!(
            spy.turn_on_count(),
            0,
            "matching apply_room_command should keep periodic dedupe cache hot"
        );
    }

    #[test]
    fn same_room_dispatches_remain_serialized() {
        let (runtime, spy) = spy_runtime();
        runtime.add_room("room-a", "Room A");
        runtime.restore_room_state("room-a", restored_active_room_state());
        spy.set_any_lights_on(true);
        spy.set_turn_on_delay_for_room("room-a", Duration::from_millis(250));

        let runtime_for_thread = runtime.clone();
        let background = std::thread::spawn(move || {
            RuntimeHandle::periodic_tick_node(runtime_for_thread.as_ref(), "room-a", "room-a", 14.0)
        });

        wait_for_any_lights_on_call(&spy, "room-a");

        let start = Instant::now();
        RuntimeHandle::turn_on_room(runtime.as_ref(), "room-a").unwrap();
        let elapsed = start.elapsed();

        background.join().unwrap().unwrap();

        assert!(
            elapsed >= Duration::from_millis(200),
            "same-room turn_on_room should wait behind in-flight periodic dispatch, elapsed {:?}",
            elapsed
        );
        assert_eq!(spy.turn_on_count(), 2);
    }
}
