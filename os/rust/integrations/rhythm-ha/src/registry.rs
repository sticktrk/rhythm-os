//! HA device registry — re-exports from rhythm-os.
//!
//! The concrete `HubDeviceRegistry` now lives in `rhythm_os::registry`.
//! This module keeps HA-specific names for the shared registry types.

pub use rhythm_os::registry::HubDeviceRegistry as HaDeviceRegistry;
pub use rhythm_os::registry::RegistrySnapshot as HaRegistrySnapshot;
pub use rhythm_os::registry::SnapshotRoom;
