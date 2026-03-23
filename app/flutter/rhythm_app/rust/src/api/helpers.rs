//! Device and area helper functions.

/// Get the Philips Hue switch/button prefixes.
///
/// These prefixes identify Hue remote controls (switches and buttons).
///
/// # Returns
///
/// List of Hue switch/button prefixes
pub fn get_hue_switch_prefixes() -> Vec<String> {
    rhythm_core::device::ieee::HUE_SWITCH_PREFIXES
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Get the Philips Hue OUI prefix.
///
/// This prefix identifies Philips Hue devices by their IEEE address.
///
/// # Returns
///
/// The Hue OUI prefix: "00:17:88:01:09"
#[allow(deprecated)]
pub fn get_hue_oui_prefix() -> String {
    rhythm_core::device::ieee::HUE_OUI_PREFIX.to_string()
}

/// Get the Rhythm group prefix.
///
/// Groups created by Rhythm use this prefix.
///
/// # Returns
///
/// The Rhythm group prefix: "Rhythm_"
pub fn get_group_prefix() -> String {
    rhythm_core::groups::GROUP_PREFIX.to_string()
}

/// Normalize an IEEE address.
///
/// Converts to lowercase with colons between byte pairs.
/// Handles various input formats (with/without colons/hyphens).
///
/// # Arguments
///
/// * `ieee` - The IEEE address string to normalize
///
/// # Returns
///
/// The normalized IEEE address (lowercase, colon-separated)
///
/// # Example
///
/// ```ignore
/// let normalized = normalize_ieee("00:17:88:01:09:AB:CD:EF".to_string());
/// assert_eq!(normalized, "00:17:88:01:09:ab:cd:ef");
/// ```
pub fn normalize_ieee(ieee: String) -> String {
    rhythm_core::device::ieee::normalize(&ieee)
}

/// Check if an IEEE address belongs to a Philips Hue device.
///
/// Detects Hue devices by checking if the IEEE address starts with
/// the Philips Hue OUI prefix (00:17:88:01:09).
///
/// # Arguments
///
/// * `ieee` - The IEEE address string to check
///
/// # Returns
///
/// True if this is a Hue device, false otherwise
///
/// # Example
///
/// ```ignore
/// assert!(is_hue_ieee("00:17:88:01:09:AB:CD:EF".to_string()));
/// assert!(!is_hue_ieee("00:11:22:33:44:55:66:77".to_string()));
/// ```
pub fn is_hue_ieee(ieee: String) -> bool {
    rhythm_core::device::ieee::is_hue(&ieee)
}

/// Get the ZigBee endpoint for a manufacturer.
///
/// Different manufacturers use different ZigBee endpoints:
/// - Philips/Signify/Hue: endpoint 11
/// - IKEA: endpoint 1
/// - Others: endpoint 11 (default)
///
/// # Arguments
///
/// * `manufacturer` - The manufacturer name
/// * `model` - The model name
///
/// # Returns
///
/// The endpoint number to use (1 or 11)
///
/// # Example
///
/// ```ignore
/// assert_eq!(endpoint_for_manufacturer("Signify Netherlands B.V.".to_string(), "LCT015".to_string()), 11);
/// assert_eq!(endpoint_for_manufacturer("IKEA of Sweden".to_string(), "TRADFRI bulb".to_string()), 1);
/// ```
pub fn endpoint_for_manufacturer(manufacturer: String, model: String) -> i32 {
    rhythm_core::device::endpoint::for_manufacturer(&manufacturer, &model) as i32
}

/// Normalize an area ID for consistent comparison.
///
/// Converts to lowercase and replaces spaces/hyphens with underscores.
///
/// # Arguments
///
/// * `area_id` - The area ID to normalize
///
/// # Returns
///
/// The normalized area ID
///
/// # Example
///
/// ```ignore
/// assert_eq!(normalize_area_id("Living Room".to_string()), "living_room");
/// assert_eq!(normalize_area_id("living-room".to_string()), "living_room");
/// ```
pub fn normalize_area_id(area_id: String) -> String {
    rhythm_core::device::area::normalize_id(&area_id)
}

/// Check if two area IDs match after normalization.
///
/// # Arguments
///
/// * `area_id_a` - First area ID
/// * `area_id_b` - Second area ID
///
/// # Returns
///
/// True if the normalized IDs match
///
/// # Example
///
/// ```ignore
/// assert!(area_ids_match("Living Room".to_string(), "living_room".to_string()));
/// assert!(!area_ids_match("living_room".to_string(), "bedroom".to_string()));
/// ```
pub fn area_ids_match(area_id_a: String, area_id_b: String) -> bool {
    rhythm_core::device::area::ids_match(&area_id_a, &area_id_b)
}

/// Generate a group name for an area.
///
/// Combines the Rhythm prefix with the area name, replacing spaces with underscores.
///
/// # Arguments
///
/// * `area_name` - The area name
///
/// # Returns
///
/// The group name (e.g., "Rhythm_Living_Room")
///
/// # Example
///
/// ```ignore
/// assert_eq!(group_name_for_area("Living Room".to_string()), "Rhythm_Living_Room");
/// ```
pub fn group_name_for_area(area_name: String) -> String {
    rhythm_core::groups::group_name_for_area(&area_name)
}

/// Check if an entity ID is a light entity.
///
/// # Arguments
///
/// * `entity_id` - The entity ID to check
///
/// # Returns
///
/// True if this is a light entity (starts with "light.")
///
/// # Example
///
/// ```ignore
/// assert!(is_light_entity("light.living_room".to_string()));
/// assert!(!is_light_entity("switch.living_room".to_string()));
/// ```
pub fn is_light_entity(entity_id: String) -> bool {
    rhythm_core::groups::is_light_entity(&entity_id)
}

/// Check if a group name is a Rhythm-managed group.
///
/// # Arguments
///
/// * `group_name` - The group name to check
///
/// # Returns
///
/// True if this is a Rhythm-managed group (starts with "Rhythm_")
///
/// # Example
///
/// ```ignore
/// assert!(is_rhythm_group("Rhythm_Living_Room".to_string()));
/// assert!(!is_rhythm_group("User_Group".to_string()));
/// ```
pub fn is_rhythm_group(group_name: String) -> bool {
    rhythm_core::groups::is_rhythm_group(&group_name)
}
