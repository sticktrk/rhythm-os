//! Area ID normalization utilities.
//!
//! Home Assistant area IDs can have various formats. This module
//! provides normalization functions for consistent comparison.

/// Normalize an area ID for consistent comparison.
///
/// Converts to lowercase and replaces spaces and hyphens with underscores.
///
/// # Examples
///
/// ```
/// use rhythm_core::device::area::normalize_id;
///
/// assert_eq!(normalize_id("Living Room"), "living_room");
/// assert_eq!(normalize_id("living-room"), "living_room");
/// assert_eq!(normalize_id("LIVING_ROOM"), "living_room");
/// ```
pub fn normalize_id(id: &str) -> String {
    id.trim().to_lowercase().replace([' ', '-'], "_")
}

/// Check if two area IDs match after normalization.
///
/// # Examples
///
/// ```
/// use rhythm_core::device::area::ids_match;
///
/// assert!(ids_match("Living Room", "living_room"));
/// assert!(ids_match("living-room", "living_room"));
/// assert!(!ids_match("living_room", "bedroom"));
/// ```
pub fn ids_match(a: &str, b: &str) -> bool {
    normalize_id(a) == normalize_id(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_id() {
        assert_eq!(normalize_id("Living Room"), "living_room");
        assert_eq!(normalize_id("living-room"), "living_room");
        assert_eq!(normalize_id("LIVING_ROOM"), "living_room");
        assert_eq!(normalize_id("  living room  "), "living_room");
        assert_eq!(normalize_id("Living-Room 2"), "living_room_2");
    }

    #[test]
    fn test_ids_match() {
        assert!(ids_match("Living Room", "living_room"));
        assert!(ids_match("living-room", "living_room"));
        assert!(ids_match("LIVING_ROOM", "living_room"));
        assert!(ids_match("Living Room", "Living-Room"));

        assert!(!ids_match("living_room", "bedroom"));
        assert!(!ids_match("living_room", "living_room_2"));
    }
}
