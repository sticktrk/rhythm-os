//! HA device registry — re-exports from rhythm-os.
//!
//! The concrete `HubDeviceRegistry` now lives in `rhythm_os::registry`.
//! This module provides backward-compatible type aliases so existing code
//! can continue to `use crate::registry::HaDeviceRegistry`.

pub use rhythm_os::registry::HubDeviceRegistry as HaDeviceRegistry;
pub use rhythm_os::registry::RegistrySnapshot as HaRegistrySnapshot;
pub use rhythm_os::registry::SnapshotRoom;
