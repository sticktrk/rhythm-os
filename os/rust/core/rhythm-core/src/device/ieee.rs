//! IEEE address handling utilities.
//!
//! Provides functions for normalizing and identifying IEEE addresses,
//! particularly for detecting Philips Hue devices.

/// Philips Hue switch/button prefixes.
///
/// These prefixes identify Hue remote controls (not bulbs):
/// - `00:17:88:01:09` - Hue dimmer switches
/// - `00:17:88:01:0b` - Hue buttons
pub const HUE_SWITCH_PREFIXES: &[&str] = &[
    "00:17:88:01:09", // Hue dimmer switches
    "00:17:88:01:0b", // Hue buttons
];

/// Normalize an IEEE address to lowercase with colons.
///
/// Handles various input formats:
/// - With colons: "00:17:88:01:09:AB:CD:EF"
/// - With hyphens: "00-17-88-01-09-AB-CD-EF"
/// - Without separators: "001788010ABCDEF"
///
/// # Examples
///
/// ```
/// use rhythm_core::device::ieee::normalize;
///
/// assert_eq!(normalize("00:17:88:01:09:AB:CD:EF"), "00:17:88:01:09:ab:cd:ef");
/// assert_eq!(normalize("00-17-88-01-09-AB-CD-EF"), "00:17:88:01:09:ab:cd:ef");
/// assert_eq!(normalize("  00:17:88:01:09:AB:CD:EF  "), "00:17:88:01:09:ab:cd:ef");
/// ```
pub fn normalize(ieee: &str) -> String {
    let trimmed = ieee.trim().to_lowercase();

    // Remove any existing separators
    let hex_only: String = trimmed.chars().filter(|c| c.is_ascii_hexdigit()).collect();

    // If we have the right length, add colons
    if hex_only.len() >= 2 {
        hex_only
            .as_bytes()
            .chunks(2)
            .map(|chunk| std::str::from_utf8(chunk).unwrap_or(""))
            .collect::<Vec<_>>()
            .join(":")
    } else {
        hex_only
    }
}

/// Check if an IEEE address belongs to a Philips Hue switch/button.
///
/// Detects Hue remote controls by checking if the IEEE address starts with
/// any of the known Hue switch/button prefixes.
///
/// # Examples
///
/// ```
/// use rhythm_core::device::ieee::is_hue;
///
/// // Hue dimmer switch
/// assert!(is_hue("00:17:88:01:09:AB:CD:EF"));
/// assert!(is_hue("00-17-88-01-09-AB-CD-EF"));
/// // Hue button
/// assert!(is_hue("00:17:88:01:0b:a0:1f:8b"));
/// // Non-Hue
/// assert!(!is_hue("00:11:22:33:44:55:66:77"));
/// ```
pub fn is_hue(ieee: &str) -> bool {
    let normalized = normalize(ieee);
    HUE_SWITCH_PREFIXES
        .iter()
        .any(|prefix| normalized.starts_with(&prefix.to_lowercase()))
}

/// Extract ZHA IEEE address from Home Assistant device identifiers.
///
/// Home Assistant device identifiers are formatted as:
/// `[["zha", "xx:xx:xx:xx:xx:xx:xx:xx"], ...]`
///
/// Returns the IEEE address if a ZHA identifier is found.
///
/// # Examples
///
/// ```
/// use rhythm_core::device::ieee::extract_from_identifiers;
///
/// let identifiers = vec![
///     vec!["zha".to_string(), "00:17:88:01:09:ab:cd:ef".to_string()],
/// ];
/// assert_eq!(
///     extract_from_identifiers(&identifiers),
///     Some("00:17:88:01:09:ab:cd:ef".to_string())
/// );
///
/// let no_zha = vec![
///     vec!["other".to_string(), "some_id".to_string()],
/// ];
/// assert_eq!(extract_from_identifiers(&no_zha), None);
/// ```
pub fn extract_from_identifiers(identifiers: &[Vec<String>]) -> Option<String> {
    for identifier in identifiers {
        if identifier.len() >= 2 && identifier[0] == "zha" {
            return Some(identifier[1].clone());
        }
    }
    None
}

/// Check if device identifiers contain a ZHA identifier.
///
/// # Examples
///
/// ```
/// use rhythm_core::device::ieee::has_zha_identifier;
///
/// let with_zha = vec![
///     vec!["zha".to_string(), "00:17:88:01:09:ab:cd:ef".to_string()],
/// ];
/// assert!(has_zha_identifier(&with_zha));
///
/// let without_zha = vec![
///     vec!["other".to_string(), "some_id".to_string()],
/// ];
/// assert!(!has_zha_identifier(&without_zha));
/// ```
pub fn has_zha_identifier(identifiers: &[Vec<String>]) -> bool {
    identifiers
        .iter()
        .any(|id| id.first().map(|s| s.as_str()) == Some("zha"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize() {
        // With colons
        assert_eq!(
            normalize("00:17:88:01:09:AB:CD:EF"),
            "00:17:88:01:09:ab:cd:ef"
        );

        // With hyphens
        assert_eq!(
            normalize("00-17-88-01-09-AB-CD-EF"),
            "00:17:88:01:09:ab:cd:ef"
        );

        // No separators
        assert_eq!(normalize("00178801090ABCDEF"), "00:17:88:01:09:0a:bc:de:f");

        // With whitespace
        assert_eq!(
            normalize("  00:17:88:01:09:AB:CD:EF  "),
            "00:17:88:01:09:ab:cd:ef"
        );

        // Already lowercase
        assert_eq!(
            normalize("00:17:88:01:09:ab:cd:ef"),
            "00:17:88:01:09:ab:cd:ef"
        );
    }

    #[test]
    fn test_is_hue() {
        // Valid Hue IEEE
        assert!(is_hue("00:17:88:01:09:AB:CD:EF"));
        assert!(is_hue("00:17:88:01:09:00:00:00"));
        assert!(is_hue("00-17-88-01-09-AB-CD-EF"));

        // Non-Hue IEEE
        assert!(!is_hue("00:11:22:33:44:55:66:77"));
        assert!(!is_hue("00:17:88:01:00:AB:CD:EF")); // Different 5th byte
        assert!(!is_hue(""));
    }

    #[test]
    fn test_extract_from_identifiers() {
        let zha_identifiers = vec![vec![
            "zha".to_string(),
            "00:17:88:01:09:ab:cd:ef".to_string(),
        ]];
        assert_eq!(
            extract_from_identifiers(&zha_identifiers),
            Some("00:17:88:01:09:ab:cd:ef".to_string())
        );

        let mixed_identifiers = vec![
            vec!["other".to_string(), "id1".to_string()],
            vec!["zha".to_string(), "00:17:88:01:09:ab:cd:ef".to_string()],
        ];
        assert_eq!(
            extract_from_identifiers(&mixed_identifiers),
            Some("00:17:88:01:09:ab:cd:ef".to_string())
        );

        let no_zha = vec![vec!["other".to_string(), "id1".to_string()]];
        assert_eq!(extract_from_identifiers(&no_zha), None);

        let empty: Vec<Vec<String>> = vec![];
        assert_eq!(extract_from_identifiers(&empty), None);
    }

    #[test]
    fn test_has_zha_identifier() {
        let with_zha = vec![vec![
            "zha".to_string(),
            "00:17:88:01:09:ab:cd:ef".to_string(),
        ]];
        assert!(has_zha_identifier(&with_zha));

        let without_zha = vec![vec!["other".to_string(), "id1".to_string()]];
        assert!(!has_zha_identifier(&without_zha));

        let empty: Vec<Vec<String>> = vec![];
        assert!(!has_zha_identifier(&empty));
    }
}
