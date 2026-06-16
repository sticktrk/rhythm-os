//! Room state persistence abstraction.
//!
//! This module provides a platform-agnostic trait for persisting room state,
//! allowing both async and blocking runtimes to share the same `RoomManager` type
//! while using different storage backends.

use crate::room::RoomManager;

/// Storage operation errors.
#[derive(Debug, Clone)]
pub enum StorageError {
    /// Storage is not available on this platform.
    NotAvailable(String),
    /// Failed to save data.
    SaveFailed(String),
    /// Failed to load data.
    LoadFailed(String),
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageError::NotAvailable(msg) => write!(f, "Storage not available: {}", msg),
            StorageError::SaveFailed(msg) => write!(f, "Save failed: {}", msg),
            StorageError::LoadFailed(msg) => write!(f, "Load failed: {}", msg),
        }
    }
}

impl std::error::Error for StorageError {}

/// Result type for storage operations.
pub type StorageResult<T> = Result<T, StorageError>;

/// Synchronous trait for persisting room state.
///
/// This trait is intentionally synchronous to work on both std and no_std
/// environments used by blocking runtimes.
///
/// # Example Implementation
///
/// ```ignore
/// use rhythm_core::runtime::storage::{RoomStateStore, StorageResult};
/// use crate::room::RoomManager;
///
/// struct JsonFileStore {
///     path: PathBuf,
/// }
///
/// impl RoomStateStore for JsonFileStore {
///     fn load_rooms(&self) -> StorageResult<RoomManager> {
///         let contents = std::fs::read_to_string(&self.path)?;
///         serde_json::from_str(&contents).map_err(|e| ...)
///     }
///
///     fn save_rooms(&self, rooms: &RoomManager) -> StorageResult<()> {
///         let json = serde_json::to_string_pretty(rooms)?;
///         std::fs::write(&self.path, json)?;
///         Ok(())
///     }
///
///     fn is_available(&self) -> bool {
///         self.path.parent().map(|p| p.exists()).unwrap_or(false)
///     }
/// }
/// ```
pub trait RoomStateStore: Send + Sync {
    /// Load room state from storage.
    ///
    /// Returns an empty `RoomManager` if no data exists yet.
    fn load_rooms(&self) -> StorageResult<RoomManager>;

    /// Save room state to storage.
    fn save_rooms(&self, rooms: &RoomManager) -> StorageResult<()>;

    /// Check if storage is available.
    ///
    /// Returns `false` if storage is not configured or unavailable.
    fn is_available(&self) -> bool;
}

/// No-op implementation for testing or when persistence isn't needed.
#[derive(Debug, Default, Clone)]
pub struct NoOpRoomStateStore;

impl RoomStateStore for NoOpRoomStateStore {
    fn load_rooms(&self) -> StorageResult<RoomManager> {
        Ok(RoomManager::new())
    }

    fn save_rooms(&self, _rooms: &RoomManager) -> StorageResult<()> {
        Ok(())
    }

    fn is_available(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_op_store() {
        let store = NoOpRoomStateStore;

        // Should return empty manager
        let rooms = store.load_rooms().unwrap();
        assert!(rooms.is_empty());

        // Save should succeed silently
        assert!(store.save_rooms(&rooms).is_ok());

        // Should report as unavailable
        assert!(!store.is_available());
    }

    #[test]
    fn test_storage_error_display() {
        let err = StorageError::SaveFailed("disk full".to_string());
        assert!(err.to_string().contains("disk full"));

        let err = StorageError::LoadFailed("corrupt data".to_string());
        assert!(err.to_string().contains("corrupt data"));

        let err = StorageError::NotAvailable("no NVS".to_string());
        assert!(err.to_string().contains("no NVS"));
    }
}
