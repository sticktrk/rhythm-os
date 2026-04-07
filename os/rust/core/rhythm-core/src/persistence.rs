//! Persistence abstractions for Rhythm OS.
//!
//! This module provides a trait for persisting room state and configuration,
//! enabling state to survive restarts across different platforms.
//!
//! Implementations:
//! - Home Assistant addon: File-based JSON storage
//! - ESP32: Flash/NVS storage (future)

use std::collections::BTreeMap;

use async_trait::async_trait;
use thiserror::Error;

use crate::room::RoomManager;
use crate::{
    default_idle_profile, default_rhythm_profile, default_sleep_profile, LightProfileConfig,
};

/// Errors that can occur with persistence operations.
#[derive(Error, Debug)]
pub enum PersistenceError {
    /// Failed to save state.
    #[error("Failed to save: {0}")]
    SaveFailed(String),

    /// Failed to load state.
    #[error("Failed to load: {0}")]
    LoadFailed(String),

    /// Storage not available.
    #[error("Storage not available: {0}")]
    NotAvailable(String),

    /// Data corruption or invalid format.
    #[error("Invalid data: {0}")]
    InvalidData(String),
}

/// Result type for persistence operations.
pub type PersistenceResult<T> = Result<T, PersistenceError>;

/// Trait for persisting Rhythm state.
///
/// This trait abstracts the storage mechanism, allowing different platforms
/// to implement persistence using their native storage:
/// - File system (HA addon)
/// - Flash/NVS (ESP32)
/// - Database (future cloud sync)
///
/// # Example
///
/// ```ignore
/// use rhythm_core::persistence::PersistenceProvider;
///
/// async fn save_state(provider: &impl PersistenceProvider, rooms: &RoomManager) {
///     if let Err(e) = provider.save_rooms(rooms).await {
///         eprintln!("Failed to save: {}", e);
///     }
/// }
/// ```
#[async_trait]
pub trait PersistenceProvider: Send + Sync {
    /// Save room states to persistent storage.
    async fn save_rooms(&self, rooms: &RoomManager) -> PersistenceResult<()>;

    /// Load room states from persistent storage.
    ///
    /// Returns an empty RoomManager if no saved state exists.
    async fn load_rooms(&self) -> PersistenceResult<RoomManager>;

    /// Save light profile configurations to persistent storage.
    async fn save_light_profiles(
        &self,
        profiles: &BTreeMap<String, LightProfileConfig>,
    ) -> PersistenceResult<()>;

    /// Load light profile configurations from persistent storage.
    ///
    /// Returns built-in defaults if no saved config exists.
    async fn load_light_profiles(&self) -> PersistenceResult<BTreeMap<String, LightProfileConfig>>;

    /// Check if persistence is available.
    ///
    /// Returns false if the storage backend is unavailable
    /// (e.g., flash full, file system read-only).
    async fn is_available(&self) -> bool;

    /// Mark that state has changed and should be saved.
    ///
    /// Implementations may use this for debounced/batched saves.
    async fn mark_dirty(&self);

    /// Save if there are pending changes.
    ///
    /// Implementations should track dirty state and only save when needed.
    async fn save_if_dirty(
        &self,
        rooms: &RoomManager,
        profiles: &BTreeMap<String, LightProfileConfig>,
    ) -> PersistenceResult<()>;
}

fn default_light_profiles() -> BTreeMap<String, LightProfileConfig> {
    let mut profiles = BTreeMap::new();
    for profile in [
        default_rhythm_profile(),
        default_sleep_profile(),
        default_idle_profile(),
    ] {
        profiles.insert(profile.id.clone(), profile);
    }
    profiles
}

/// A no-op persistence provider for testing or when persistence isn't needed.
#[derive(Debug, Default)]
pub struct NoOpPersistenceProvider;

impl NoOpPersistenceProvider {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl PersistenceProvider for NoOpPersistenceProvider {
    async fn save_rooms(&self, _rooms: &RoomManager) -> PersistenceResult<()> {
        Ok(())
    }

    async fn load_rooms(&self) -> PersistenceResult<RoomManager> {
        Ok(RoomManager::new())
    }

    async fn save_light_profiles(
        &self,
        _profiles: &BTreeMap<String, LightProfileConfig>,
    ) -> PersistenceResult<()> {
        Ok(())
    }

    async fn load_light_profiles(&self) -> PersistenceResult<BTreeMap<String, LightProfileConfig>> {
        Ok(default_light_profiles())
    }

    async fn is_available(&self) -> bool {
        false
    }

    async fn mark_dirty(&self) {
        // No-op
    }

    async fn save_if_dirty(
        &self,
        _rooms: &RoomManager,
        _profiles: &BTreeMap<String, LightProfileConfig>,
    ) -> PersistenceResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_noop_provider() {
        let provider = NoOpPersistenceProvider::new();

        assert!(!provider.is_available().await);

        let rooms = provider.load_rooms().await.unwrap();
        assert!(rooms.is_empty());

        let profiles = provider.load_light_profiles().await.unwrap();
        assert!(profiles.contains_key(crate::RHYTHM_PROFILE_ID));
        assert!(profiles.contains_key(crate::SLEEP_PROFILE_ID));
        assert!(profiles.contains_key(crate::IDLE_PROFILE_ID));

        // Save operations should succeed silently
        provider.save_rooms(&RoomManager::new()).await.unwrap();
        provider
            .save_light_profiles(&default_light_profiles())
            .await
            .unwrap();
    }
}
