//! Light group management.
//!
//! This module provides abstractions for managing groups of lights,
//! enabling efficient group control instead of individual light commands.
//!
//! Groups are used by both:
//! - Home Assistant (ZHA groups via WebSocket)
//! - Direct ZigBee (native ZigBee group addressing)

use async_trait::async_trait;
use thiserror::Error;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

// =============================================================================
// Group naming constants and helpers
// =============================================================================

/// Prefix for Rhythm-managed ZHA groups.
///
/// Groups created by Rhythm use this prefix to distinguish them from
/// user-created groups and to enable automatic cleanup/management.
pub const GROUP_PREFIX: &str = "Rhythm_";

/// Generate a group name for an area.
///
/// Combines the Rhythm prefix with the area name, replacing spaces with underscores.
///
/// # Examples
///
/// ```
/// use rhythm_core::groups::group_name_for_area;
///
/// assert_eq!(group_name_for_area("Living Room"), "Rhythm_Living_Room");
/// assert_eq!(group_name_for_area("Bedroom"), "Rhythm_Bedroom");
/// ```
pub fn group_name_for_area(area_name: &str) -> String {
    format!("{}{}", GROUP_PREFIX, area_name.replace(' ', "_"))
}

/// Check if an entity ID looks like a light entity.
///
/// # Examples
///
/// ```
/// use rhythm_core::groups::is_light_entity;
///
/// assert!(is_light_entity("light.living_room"));
/// assert!(!is_light_entity("switch.living_room"));
/// ```
pub fn is_light_entity(entity_id: &str) -> bool {
    entity_id.starts_with("light.")
}

/// Check if a group name is a Rhythm-managed group.
///
/// # Examples
///
/// ```
/// use rhythm_core::groups::is_rhythm_group;
///
/// assert!(is_rhythm_group("Rhythm_Living_Room"));
/// assert!(!is_rhythm_group("User_Group"));
/// ```
pub fn is_rhythm_group(name: &str) -> bool {
    name.starts_with(GROUP_PREFIX)
}

// =============================================================================
// Group types and traits
// =============================================================================

/// Errors that can occur with group operations.
#[derive(Error, Debug)]
pub enum GroupError {
    /// Failed to create a group.
    #[error("Failed to create group: {0}")]
    CreateFailed(String),

    /// Failed to sync groups.
    #[error("Failed to sync groups: {0}")]
    SyncFailed(String),

    /// Group not found.
    #[error("Group not found for area: {0}")]
    NotFound(String),

    /// Backend not available (e.g., ZHA not installed).
    #[error("Group backend not available: {0}")]
    NotAvailable(String),

    /// Communication error.
    #[error("Communication error: {0}")]
    Communication(String),
}

/// Result type for group operations.
pub type GroupResult<T> = Result<T, GroupError>;

/// A group of lights that can be controlled together.
///
/// Groups enable efficient control of multiple lights with a single command,
/// which is especially important for ZigBee where group addressing is a
/// native protocol feature.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct LightGroup {
    /// Unique identifier for the group (platform-specific format).
    pub id: String,

    /// Human-readable name for the group.
    pub name: String,

    /// The area this group is associated with.
    pub area_id: String,

    /// Entity ID or address used to control the group.
    /// For ZHA: "light.rhythm_living_room"
    /// For ZigBee: group address (u16)
    pub control_id: String,

    /// Member identifiers (IEEE addresses, entity IDs, etc.).
    pub members: Vec<String>,
}

impl LightGroup {
    /// Create a new light group.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        area_id: impl Into<String>,
        control_id: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            area_id: area_id.into(),
            control_id: control_id.into(),
            members: Vec::new(),
        }
    }

    /// Create a new light group with members.
    pub fn with_members(
        id: impl Into<String>,
        name: impl Into<String>,
        area_id: impl Into<String>,
        control_id: impl Into<String>,
        members: Vec<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            area_id: area_id.into(),
            control_id: control_id.into(),
            members,
        }
    }
}

/// Trait for managing light groups.
///
/// Implementations exist for different backends:
/// - `ZhaGroupManager` - Home Assistant ZHA groups via WebSocket
/// - Future: `ZigBeeGroupManager` - Direct ZigBee group control
///
/// # Example
///
/// ```ignore
/// use rhythm_core::groups::GroupController;
///
/// async fn setup_groups(controller: &impl GroupController) {
///     // Sync groups with the backend
///     if let Ok(groups) = controller.sync_groups().await {
///         for group in groups {
///             println!("Group {} has {} members", group.name, group.members.len());
///         }
///     }
/// }
/// ```
#[async_trait]
pub trait GroupController: Send + Sync {
    /// Synchronize groups with the backend.
    ///
    /// This creates or updates groups for areas that have multiple lights,
    /// enabling efficient group control.
    ///
    /// Returns the list of synced groups.
    async fn sync_groups(&self) -> GroupResult<Vec<LightGroup>>;

    /// Get the group for an area, if one exists.
    async fn get_group_for_area(&self, area_id: &str) -> Option<LightGroup>;

    /// Get all managed groups.
    async fn get_all_groups(&self) -> Vec<LightGroup>;

    /// Check if group control is available.
    ///
    /// Returns false if the backend doesn't support groups
    /// (e.g., ZHA not installed, no ZigBee coordinator).
    async fn is_available(&self) -> bool;
}

/// A no-op group controller for testing or when groups aren't available.
#[derive(Debug, Default)]
pub struct NoOpGroupController;

impl NoOpGroupController {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl GroupController for NoOpGroupController {
    async fn sync_groups(&self) -> GroupResult<Vec<LightGroup>> {
        Ok(vec![])
    }

    async fn get_group_for_area(&self, _area_id: &str) -> Option<LightGroup> {
        None
    }

    async fn get_all_groups(&self) -> Vec<LightGroup> {
        vec![]
    }

    async fn is_available(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_group_prefix() {
        assert_eq!(GROUP_PREFIX, "Rhythm_");
    }

    #[test]
    fn test_group_name_for_area() {
        assert_eq!(group_name_for_area("Living Room"), "Rhythm_Living_Room");
        assert_eq!(group_name_for_area("Bedroom"), "Rhythm_Bedroom");
        assert_eq!(group_name_for_area("Office 2"), "Rhythm_Office_2");
    }

    #[test]
    fn test_is_light_entity() {
        assert!(is_light_entity("light.living_room"));
        assert!(is_light_entity("light.rhythm_bedroom"));
        assert!(!is_light_entity("switch.living_room"));
        assert!(!is_light_entity("sensor.temperature"));
    }

    #[test]
    fn test_is_rhythm_group() {
        assert!(is_rhythm_group("Rhythm_Living_Room"));
        assert!(is_rhythm_group("Rhythm_Bedroom"));
        assert!(!is_rhythm_group("User_Group"));
        assert!(!is_rhythm_group("living_room"));
    }

    #[test]
    fn test_light_group_new() {
        let group = LightGroup::new(
            "grp_1",
            "Living Room",
            "living_room",
            "light.rhythm_living_room",
        );
        assert_eq!(group.id, "grp_1");
        assert_eq!(group.name, "Living Room");
        assert_eq!(group.area_id, "living_room");
        assert_eq!(group.control_id, "light.rhythm_living_room");
        assert!(group.members.is_empty());
    }

    #[test]
    fn test_light_group_with_members() {
        let members = vec!["00:11:22:33".to_string(), "44:55:66:77".to_string()];
        let group = LightGroup::with_members(
            "grp_1",
            "Living Room",
            "living_room",
            "light.rhythm_living_room",
            members.clone(),
        );
        assert_eq!(group.members, members);
    }

    #[tokio::test]
    async fn test_noop_controller() {
        let controller = NoOpGroupController::new();

        assert!(!controller.is_available().await);
        assert!(controller.get_all_groups().await.is_empty());
        assert!(controller.get_group_for_area("test").await.is_none());

        let groups = controller.sync_groups().await.unwrap();
        assert!(groups.is_empty());
    }
}
