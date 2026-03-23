//! Error types for the runtime module.

use thiserror::Error;

/// Result type alias for runtime operations.
pub type RuntimeResult<T> = Result<T, RuntimeError>;

/// Errors that can occur in the runtime.
#[derive(Error, Debug)]
pub enum RuntimeError {
    /// Room not found.
    #[error("Room not found: {0}")]
    RoomNotFound(String),

    /// Device not found in registry.
    #[error("Device not found: {0}")]
    DeviceNotFound(String),

    /// Scheduler error.
    #[error("Scheduler error: {0}")]
    SchedulerError(String),

    /// Light control error.
    #[error("Light control error: {0}")]
    LightControlError(#[from] crate::LightControlError),

    /// Persistence error.
    #[error("Persistence error: {0}")]
    PersistenceError(#[from] crate::PersistenceError),

    /// Configuration error.
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Internal error.
    #[error("Internal error: {0}")]
    Internal(String),
}
