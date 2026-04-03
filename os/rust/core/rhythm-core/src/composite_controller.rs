//! Composite light controller for multi-hub fan-out.
//!
//! Wraps multiple per-hub [`LightController`] implementations and routes
//! commands to the correct hub(s) based on a room routing table. Supports
//! dynamic registration — controllers can be added/removed while the
//! `RhythmEngine` is running.
//!
//! ## Usage
//!
//! ```ignore
//! let composite = CompositeController::new();
//! composite.register_controller("hue@192.168.1.5", hue_controller);
//! composite.register_controller("hue@192.168.1.6", hue_controller_2);
//! composite.update_routing(hashmap! {
//!     "living-room" => vec!["hue@192.168.1.5"],
//!     "kitchen"     => vec!["hue@192.168.1.6"],
//!     "hallway"     => vec!["hue@192.168.1.5", "hue@192.168.1.6"],
//! });
//! // Now turn_on("hallway", cmd) fans out to both bridges.
//! ```

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use log::warn;

use crate::controller::{LightControlError, LightControlResult, LightController};
use crate::lighting::LightingCommand;
use crate::room::Room;

/// Block on a future from a non-async context (spawned OS threads).
#[cfg(any(feature = "tokio", feature = "blocking"))]
fn sync_block_on<F: std::future::Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

/// Minimal poll loop when no executor crate is available (tests).
#[cfg(not(any(feature = "tokio", feature = "blocking")))]
fn sync_block_on<F: std::future::Future>(f: F) -> F::Output {
    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(&waker);
    let mut f = std::pin::pin!(f);
    loop {
        match f.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(v) => return v,
            std::task::Poll::Pending => std::thread::yield_now(),
        }
    }
}

/// Dispatch an operation to all hub targets in parallel on OS threads.
///
/// Uses `std::thread::scope` so each hub runs on its own thread — a slow hub
/// (e.g. Matter with 10s connect timeout) doesn't block fast hubs (Hue, HA).
/// Works from any context (tokio runtime, std::thread, etc.).
///
/// For single targets, runs inline without spawning a thread.
fn dispatch_parallel<F>(
    targets: &[(String, Arc<dyn LightController>, String)],
    op: F,
) -> bool
where
    F: Fn(Arc<dyn LightController>, &str) -> LightControlResult<()> + Sync + Send,
{
    if targets.len() <= 1 {
        // Single target — no thread overhead needed
        if let Some((key, controller, hub_room_id)) = targets.first() {
            match op(controller.clone(), hub_room_id) {
                Ok(()) => return true,
                Err(e) => {
                    warn!(target: "composite", "hub {} failed: {}", key, e);
                    return false;
                }
            }
        }
        return false;
    }

    std::thread::scope(|s| {
        let op = &op;
        let handles: Vec<_> = targets
            .iter()
            .map(|(key, controller, hub_room_id)| {
                let controller = controller.clone();
                let key = key.as_str();
                let hub_room_id = hub_room_id.as_str();
                s.spawn(move || (key, op(controller, hub_room_id)))
            })
            .collect();

        let mut any_ok = false;
        for handle in handles {
            match handle.join() {
                Ok((_, Ok(()))) => any_ok = true,
                Ok((key, Err(e))) => {
                    warn!(target: "composite", "hub {} failed: {}", key, e);
                }
                Err(_) => {
                    warn!(target: "composite", "hub dispatch thread panicked");
                }
            }
        }
        any_ok
    })
}

/// A composite light controller that fans out commands to per-hub controllers.
///
/// Interior mutability via [`RwLock`] allows adding/removing controllers
/// and updating the routing table while the engine is running.
pub struct CompositeController {
    /// Per-hub controllers keyed by hub identifier (e.g., "hue@192.168.1.5").
    controllers: RwLock<HashMap<String, Arc<dyn LightController>>>,
    /// Room routing: topology_room_id → list of (hub_key, hub_room_id) pairs.
    /// The composite uses hub_room_id when calling per-hub controllers.
    routing: RwLock<HashMap<String, Vec<(String, String)>>>,
}

impl CompositeController {
    /// Create an empty composite controller (no hubs registered).
    pub fn new() -> Self {
        Self {
            controllers: RwLock::new(HashMap::new()),
            routing: RwLock::new(HashMap::new()),
        }
    }

    /// Register a per-hub controller. Replaces any existing controller for this key.
    pub fn register_controller(&self, key: &str, controller: Arc<dyn LightController>) {
        if let Ok(mut controllers) = self.controllers.write() {
            controllers.insert(key.to_string(), controller);
        }
    }

    /// Remove a per-hub controller.
    pub fn remove_controller(&self, key: &str) {
        if let Ok(mut controllers) = self.controllers.write() {
            controllers.remove(key);
        }
    }

    /// Replace the full routing table.
    ///
    /// Each entry maps a topology room ID to a list of (hub_key, hub_room_id)
    /// pairs. For single-hub rooms, the Vec has one entry. For cross-hub rooms,
    /// it has multiple. The composite calls each hub's controller with its
    /// hub-native room ID.
    pub fn update_routing(&self, table: HashMap<String, Vec<(String, String)>>) {
        if let Ok(mut routing) = self.routing.write() {
            *routing = table;
        }
    }

    /// How many hub controllers are registered.
    pub fn controller_count(&self) -> usize {
        self.controllers.read().map(|c| c.len()).unwrap_or(0)
    }

    /// Get controller targets for a room from the routing table.
    ///
    /// Returns (hub_key, controller, hub_room_id) triples. Falls back to ALL
    /// controllers with `room_id` as the hub_room_id if no routing entry exists
    /// (single-hub compat before topology sync).
    fn controllers_for_room(
        &self,
        room_id: &str,
    ) -> Vec<(String, Arc<dyn LightController>, String)> {
        let targets = self
            .routing
            .read()
            .ok()
            .and_then(|r| r.get(room_id).cloned());

        let controllers = match self.controllers.read() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        if let Some(targets) = targets {
            targets
                .iter()
                .filter_map(|(hub_key, hub_room_id)| {
                    controllers
                        .get(hub_key)
                        .map(|c| (hub_key.clone(), c.clone(), hub_room_id.clone()))
                })
                .collect()
        } else {
            // Fallback: try all controllers with room_id as-is (single-hub compat)
            warn!(target: "composite",
                "No routing entry for room '{}' — falling back to all {} controllers",
                room_id, controllers.len());
            controllers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone(), room_id.to_string()))
                .collect()
        }
    }
}

impl Default for CompositeController {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LightController for CompositeController {
    async fn turn_on(&self, room_id: &str, command: LightingCommand) -> LightControlResult<()> {
        let targets = self.controllers_for_room(room_id);
        if targets.is_empty() {
            return Err(LightControlError::RoomNotFound(format!(
                "No controllers for room '{}'",
                room_id
            )));
        }

        // Single target: await directly — avoids nested LocalPool executor on
        // periodic/event-loop std::threads where the outer executor::block_on
        // already created a LocalPool.
        if targets.len() == 1 {
            let (key, controller, hub_room_id) = &targets[0];
            return controller.turn_on(hub_room_id, command).await.map_err(|e| {
                warn!(target: "composite", "hub {} failed: {}", key, e);
                e
            });
        }

        // Multi-target: fan out to OS threads (thread::scope = fresh thread-locals, safe)
        let any_ok = dispatch_parallel(&targets, |controller, hub_room_id| {
            sync_block_on(controller.turn_on(hub_room_id, command.clone()))
        });

        if any_ok {
            Ok(())
        } else {
            Err(LightControlError::CommandFailed(format!(
                "All controllers failed for room '{}'",
                room_id
            )))
        }
    }

    async fn turn_off(&self, room_id: &str) -> LightControlResult<()> {
        let targets = self.controllers_for_room(room_id);
        if targets.is_empty() {
            return Err(LightControlError::RoomNotFound(format!(
                "No controllers for room '{}'",
                room_id
            )));
        }

        // Single target: await directly (same reasoning as turn_on)
        if targets.len() == 1 {
            let (key, controller, hub_room_id) = &targets[0];
            return controller.turn_off(hub_room_id).await.map_err(|e| {
                warn!(target: "composite", "hub {} failed: {}", key, e);
                e
            });
        }

        // Multi-target: fan out to OS threads
        let any_ok = dispatch_parallel(&targets, |controller, hub_room_id| {
            sync_block_on(controller.turn_off(hub_room_id))
        });

        if any_ok {
            Ok(())
        } else {
            Err(LightControlError::CommandFailed(format!(
                "All controllers failed for room '{}'",
                room_id
            )))
        }
    }

    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        let all_controllers: Vec<Arc<dyn LightController>> = self
            .controllers
            .read()
            .map(|c| c.values().cloned().collect())
            .unwrap_or_default();

        let mut rooms = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for controller in &all_controllers {
            if let Ok(hub_rooms) = controller.get_rooms().await {
                for room in hub_rooms {
                    if seen.insert(room.id.clone()) {
                        rooms.push(room);
                    }
                }
            }
        }
        Ok(rooms)
    }

    async fn is_connected(&self) -> bool {
        let all_controllers: Vec<Arc<dyn LightController>> = self
            .controllers
            .read()
            .map(|c| c.values().cloned().collect())
            .unwrap_or_default();

        for controller in &all_controllers {
            if controller.is_connected().await {
                return true;
            }
        }
        false
    }

    async fn any_lights_on(&self, room_id: &str) -> LightControlResult<bool> {
        let targets = self.controllers_for_room(room_id);
        if targets.is_empty() {
            return Err(LightControlError::RoomNotFound(format!(
                "No controllers for room '{}'",
                room_id
            )));
        }

        for (key, controller, hub_room_id) in &targets {
            match controller.any_lights_on(hub_room_id).await {
                Ok(true) => return Ok(true),
                Ok(false) => {}
                Err(e) => {
                    warn!(target: "composite", "any_lights_on '{}' via {}: {}", hub_room_id, key, e);
                }
            }
        }
        Ok(false)
    }

    fn name(&self) -> &str {
        "Composite"
    }
}

/// `LightController` impl for `Arc<CompositeController>` so the runtime can
/// hold a shared reference while `AppState` holds another for dynamic registration.
#[async_trait]
impl LightController for Arc<CompositeController> {
    async fn turn_on(&self, room_id: &str, command: LightingCommand) -> LightControlResult<()> {
        (**self).turn_on(room_id, command).await
    }
    async fn turn_off(&self, room_id: &str) -> LightControlResult<()> {
        (**self).turn_off(room_id).await
    }
    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        (**self).get_rooms().await
    }
    async fn is_connected(&self) -> bool {
        (**self).is_connected().await
    }
    async fn any_lights_on(&self, room_id: &str) -> LightControlResult<bool> {
        (**self).any_lights_on(room_id).await
    }
    fn name(&self) -> &str {
        (**self).name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    /// Mock controller that records calls and can be configured to fail.
    struct MockController {
        name: String,
        should_fail: AtomicBool,
        turn_on_calls: Mutex<Vec<(String, LightingCommand)>>,
        turn_off_calls: Mutex<Vec<String>>,
        lights_on: AtomicBool,
        rooms: Mutex<Vec<Room>>,
    }

    impl MockController {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                should_fail: AtomicBool::new(false),
                turn_on_calls: Mutex::new(Vec::new()),
                turn_off_calls: Mutex::new(Vec::new()),
                lights_on: AtomicBool::new(false),
                rooms: Mutex::new(Vec::new()),
            }
        }

        fn with_rooms(name: &str, rooms: Vec<Room>) -> Self {
            let c = Self::new(name);
            *c.rooms.lock().unwrap() = rooms;
            c
        }

        fn set_fail(&self, fail: bool) {
            self.should_fail.store(fail, Ordering::Relaxed);
        }

        fn set_lights_on(&self, on: bool) {
            self.lights_on.store(on, Ordering::Relaxed);
        }

        fn turn_on_count(&self) -> usize {
            self.turn_on_calls.lock().unwrap().len()
        }

        fn turn_off_count(&self) -> usize {
            self.turn_off_calls.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl LightController for MockController {
        async fn turn_on(&self, room_id: &str, command: LightingCommand) -> LightControlResult<()> {
            if self.should_fail.load(Ordering::Relaxed) {
                return Err(LightControlError::CommandFailed("mock failure".into()));
            }
            self.turn_on_calls
                .lock()
                .unwrap()
                .push((room_id.to_string(), command));
            Ok(())
        }

        async fn turn_off(&self, room_id: &str) -> LightControlResult<()> {
            if self.should_fail.load(Ordering::Relaxed) {
                return Err(LightControlError::CommandFailed("mock failure".into()));
            }
            self.turn_off_calls
                .lock()
                .unwrap()
                .push(room_id.to_string());
            Ok(())
        }

        async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
            Ok(self.rooms.lock().unwrap().clone())
        }

        async fn is_connected(&self) -> bool {
            !self.should_fail.load(Ordering::Relaxed)
        }

        async fn any_lights_on(&self, _room_id: &str) -> LightControlResult<bool> {
            if self.should_fail.load(Ordering::Relaxed) {
                return Err(LightControlError::CommandFailed("mock failure".into()));
            }
            Ok(self.lights_on.load(Ordering::Relaxed))
        }

        fn name(&self) -> &str {
            &self.name
        }
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        // Minimal block_on for tests — no tokio/futures dependency needed.
        // async_trait methods are actually synchronous (blocking transport).
        let waker = std::task::Waker::noop();
        let mut cx = std::task::Context::from_waker(waker);
        let mut f = std::pin::pin!(f);
        loop {
            match f.as_mut().poll(&mut cx) {
                std::task::Poll::Ready(v) => return v,
                std::task::Poll::Pending => {
                    // In test context with blocking transports, this shouldn't happen.
                    // If it does, we'd spin — acceptable for tests only.
                    std::thread::yield_now();
                }
            }
        }
    }

    // ── Registration ─────────────────────────────────────────────────

    #[test]
    fn empty_composite_has_no_controllers() {
        let composite = CompositeController::new();
        assert_eq!(composite.controller_count(), 0);
    }

    #[test]
    fn register_and_count() {
        let composite = CompositeController::new();
        composite.register_controller("hub_a", Arc::new(MockController::new("a")));
        assert_eq!(composite.controller_count(), 1);
        composite.register_controller("hub_b", Arc::new(MockController::new("b")));
        assert_eq!(composite.controller_count(), 2);
    }

    #[test]
    fn remove_controller() {
        let composite = CompositeController::new();
        composite.register_controller("hub_a", Arc::new(MockController::new("a")));
        composite.remove_controller("hub_a");
        assert_eq!(composite.controller_count(), 0);
    }

    // ── Single controller, single room ───────────────────────────────

    #[tokio::test]
    async fn single_controller_turn_on() {
        let mock = Arc::new(MockController::new("hub_a"));
        let composite = CompositeController::new();
        composite.register_controller("hub_a", mock.clone());
        composite.update_routing(HashMap::from([(
            "room1".to_string(),
            vec![("hub_a".to_string(), "room1".to_string())],
        )]));

        let cmd = LightingCommand::new(80, 4000);
        composite.turn_on("room1", cmd).await.unwrap();
        assert_eq!(mock.turn_on_count(), 1);
    }

    #[tokio::test]
    async fn single_controller_turn_off() {
        let mock = Arc::new(MockController::new("hub_a"));
        let composite = CompositeController::new();
        composite.register_controller("hub_a", mock.clone());
        composite.update_routing(HashMap::from([(
            "room1".to_string(),
            vec![("hub_a".to_string(), "room1".to_string())],
        )]));

        composite.turn_off("room1").await.unwrap();
        assert_eq!(mock.turn_off_count(), 1);
    }

    // ── Cross-hub fan-out ────────────────────────────────────────────

    #[tokio::test]
    async fn cross_hub_turn_on_fans_out() {
        let mock_a = Arc::new(MockController::new("hub_a"));
        let mock_b = Arc::new(MockController::new("hub_b"));

        let composite = CompositeController::new();
        composite.register_controller("hub_a", mock_a.clone());
        composite.register_controller("hub_b", mock_b.clone());
        composite.update_routing(HashMap::from([(
            "hallway".to_string(),
            vec![
                ("hub_a".to_string(), "hallway".to_string()),
                ("hub_b".to_string(), "hallway".to_string()),
            ],
        )]));

        let cmd = LightingCommand::new(60, 3500);
        composite.turn_on("hallway", cmd).await.unwrap();
        assert_eq!(mock_a.turn_on_count(), 1);
        assert_eq!(mock_b.turn_on_count(), 1);
    }

    // ── any_lights_on OR semantics ───────────────────────────────────

    #[test]
    fn any_lights_on_or_across_hubs() {
        let mock_a = Arc::new(MockController::new("hub_a"));
        let mock_b = Arc::new(MockController::new("hub_b"));
        mock_a.set_lights_on(false);
        mock_b.set_lights_on(true);

        let composite = CompositeController::new();
        composite.register_controller("hub_a", mock_a);
        composite.register_controller("hub_b", mock_b);
        composite.update_routing(HashMap::from([(
            "room1".to_string(),
            vec![
                ("hub_a".to_string(), "room1".to_string()),
                ("hub_b".to_string(), "room1".to_string()),
            ],
        )]));

        assert!(block_on(composite.any_lights_on("room1")).unwrap());
    }

    #[test]
    fn any_lights_on_all_off() {
        let mock = Arc::new(MockController::new("hub_a"));
        mock.set_lights_on(false);

        let composite = CompositeController::new();
        composite.register_controller("hub_a", mock);
        composite.update_routing(HashMap::from([(
            "room1".to_string(),
            vec![("hub_a".to_string(), "room1".to_string())],
        )]));

        assert!(!block_on(composite.any_lights_on("room1")).unwrap());
    }

    // ── Unknown room ─────────────────────────────────────────────────

    #[test]
    fn unknown_room_no_controllers_returns_error() {
        let composite = CompositeController::new();
        // No controllers registered at all
        let result = block_on(composite.turn_on("unknown", LightingCommand::new(50, 3000)));
        assert!(result.is_err());
    }

    // ── Partial failure ──────────────────────────────────────────────

    #[tokio::test]
    async fn partial_failure_still_succeeds() {
        let mock_a = Arc::new(MockController::new("hub_a"));
        let mock_b = Arc::new(MockController::new("hub_b"));
        mock_a.set_fail(true); // hub_a will fail

        let composite = CompositeController::new();
        composite.register_controller("hub_a", mock_a.clone());
        composite.register_controller("hub_b", mock_b.clone());
        composite.update_routing(HashMap::from([(
            "room1".to_string(),
            vec![
                ("hub_a".to_string(), "room1".to_string()),
                ("hub_b".to_string(), "room1".to_string()),
            ],
        )]));

        // Should succeed because hub_b works
        let cmd = LightingCommand::new(80, 4000);
        composite.turn_on("room1", cmd).await.unwrap();
        assert_eq!(mock_a.turn_on_count(), 0); // failed
        assert_eq!(mock_b.turn_on_count(), 1); // succeeded
    }

    #[tokio::test]
    async fn all_controllers_fail_returns_error() {
        let mock_a = Arc::new(MockController::new("hub_a"));
        mock_a.set_fail(true);

        let composite = CompositeController::new();
        composite.register_controller("hub_a", mock_a);
        composite.update_routing(HashMap::from([(
            "room1".to_string(),
            vec![("hub_a".to_string(), "room1".to_string())],
        )]));

        let result = composite.turn_on("room1", LightingCommand::new(50, 3000)).await;
        assert!(result.is_err());
    }

    // ── Fallback: no routing entry → all controllers ─────────────────

    #[tokio::test]
    async fn no_routing_entry_falls_back_to_all_controllers() {
        let mock = Arc::new(MockController::new("hub_a"));
        let composite = CompositeController::new();
        composite.register_controller("hub_a", mock.clone());
        // No routing set — should fall back to all controllers

        let cmd = LightingCommand::new(80, 4000);
        composite.turn_on("room1", cmd).await.unwrap();
        assert_eq!(mock.turn_on_count(), 1);
    }

    // ── get_rooms deduplicates ───────────────────────────────────────

    #[test]
    fn get_rooms_deduplicates() {
        let room = Room::new("room1", "Living Room");

        let mock_a = Arc::new(MockController::with_rooms("hub_a", vec![room.clone()]));
        let mock_b = Arc::new(MockController::with_rooms("hub_b", vec![room]));

        let composite = CompositeController::new();
        composite.register_controller("hub_a", mock_a);
        composite.register_controller("hub_b", mock_b);

        let rooms = block_on(composite.get_rooms()).unwrap();
        assert_eq!(rooms.len(), 1); // deduplicated
    }

    // ── is_connected ─────────────────────────────────────────────────

    #[test]
    fn is_connected_any_hub() {
        let mock_a = Arc::new(MockController::new("hub_a"));
        let mock_b = Arc::new(MockController::new("hub_b"));
        mock_a.set_fail(true); // disconnected
                               // mock_b is connected

        let composite = CompositeController::new();
        composite.register_controller("hub_a", mock_a);
        composite.register_controller("hub_b", mock_b);

        assert!(block_on(composite.is_connected()));
    }

    #[test]
    fn is_connected_none() {
        let composite = CompositeController::new();
        assert!(!block_on(composite.is_connected()));
    }

    // ── name ─────────────────────────────────────────────────────────

    #[test]
    fn name_is_composite() {
        let composite = CompositeController::new();
        assert_eq!(composite.name(), "Composite");
    }
}
