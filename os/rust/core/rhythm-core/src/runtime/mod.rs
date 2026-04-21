//! Runtime: platform-agnostic scheduling, events, and orchestration.

pub mod adapters;
pub mod config;
pub mod error;
pub mod events;
pub mod executor;
pub mod handle;
#[cfg(feature = "serde")]
pub mod hub_registry;
pub mod orchestrator;
pub mod registry;
pub mod scheduler;
pub mod storage;
pub mod time;

pub use config::{RoomConfig, RuntimeConfig};
pub use error::{RuntimeError, RuntimeResult};
pub use events::{ButtonAction, InputEvent, ZhaEventArgs};
pub use handle::{NodeSnapshot, RestoredNodeState, RestoredRoomState, RoomSnapshot, RuntimeHandle};
#[cfg(feature = "serde")]
pub use hub_registry::{DeviceType, HubRegistry};
pub use orchestrator::RhythmRuntime;
pub use registry::{DeviceRegistry, SimpleDeviceRegistry};
pub use scheduler::{NoOpScheduler, ScheduleHandle, Scheduler};
pub use storage::{NoOpRoomStateStore, RoomStateStore, StorageError, StorageResult};
pub use time::{MockTimeProvider, TimeProvider};
pub use adapters::{SystemTimeProvider, ThreadScheduler};
