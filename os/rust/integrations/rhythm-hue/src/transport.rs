//! Hue transport abstraction.
//!
//! Defines the `HueTransport` trait that platform-specific crates implement
//! to provide HTTP communication with the Hue bridge. The trait methods
//! mirror the public HTTP operations required by the integration.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HueBridgeSearchLight {
    /// Legacy Hue V1 light identifier returned by `/lights/new`.
    pub legacy_id: String,
    pub name: String,
}

pub fn normalize_hue_bridge_serial(raw: &str) -> anyhow::Result<String> {
    let normalized = raw
        .trim()
        .chars()
        .filter(|character| !character.is_ascii_whitespace() && *character != '-')
        .flat_map(char::to_uppercase)
        .collect::<String>();
    if normalized.len() != 6
        || !normalized
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        anyhow::bail!("Hue bulb serial must be six hexadecimal characters");
    }
    Ok(normalized)
}

/// Platform-agnostic interface to the Hue bridge HTTP API.
///
/// Implementors provide the actual HTTP/TLS transport. This can be reqwest,
/// a blocking native client, or any other HTTP library.
pub trait HueTransport: Send + Sync {
    /// Test the connection with stored credentials (lightweight check).
    fn test_connection(&self, username: &str) -> anyhow::Result<bool>;

    /// Eagerly establish the TLS connection so the first API call doesn't
    /// pay the handshake cost.
    fn warmup_tls(&self) -> anyhow::Result<()>;

    /// Control a grouped_light (room-level control) via V2 API.
    ///
    /// Uses `color_temperature.mirek` for color temp, or `color.xy` for direct color.
    /// Brightness is 0-100 (V2 API percentage via `dimming.brightness`).
    /// When `xy` is Some, sends xy color instead of color temperature.
    #[allow(clippy::too_many_arguments)]
    fn set_grouped_light(
        &self,
        username: &str,
        grouped_light_id: &str,
        on: bool,
        brightness: Option<u8>,
        kelvin: Option<u16>,
        xy: Option<(f32, f32)>,
        fade_ms: Option<u16>,
    ) -> anyhow::Result<()>;

    /// Control a single Hue light resource via V2 API.
    ///
    /// Uses the same command body shape as grouped_light control, but targets
    /// `PUT /clip/v2/resource/light/{id}`.
    #[allow(clippy::too_many_arguments)]
    fn set_light(
        &self,
        username: &str,
        light_id: &str,
        on: bool,
        brightness: Option<u8>,
        kelvin: Option<u16>,
        xy: Option<(f32, f32)>,
        fade_ms: Option<u16>,
    ) -> anyhow::Result<()>;

    /// Check if a grouped_light (room) has any lights on.
    fn is_grouped_light_on(&self, username: &str, grouped_light_id: &str) -> anyhow::Result<bool>;

    /// Check whether a single Hue light resource is on.
    fn is_light_on(&self, username: &str, light_id: &str) -> anyhow::Result<bool>;

    /// Trigger the Hue V2 native identify effect on a single light resource.
    ///
    /// Uses `PUT /clip/v2/resource/light/{id}` with `identify.action = "identify"`,
    /// which triggers Hue's physical identification sequence without
    /// perturbing the on/brightness state.
    fn identify_light(&self, username: &str, light_id: &str) -> anyhow::Result<()>;

    /// Fetch all resources of a given type from the Hue V2 API.
    ///
    /// Used by discovery to enumerate rooms, devices, and services.
    /// Returns the raw JSON response envelope.
    fn get_resources(
        &self,
        username: &str,
        resource_type: &str,
    ) -> anyhow::Result<serde_json::Value>;

    /// Ask a connected Hue Bridge to search its Zigbee network for the bulb
    /// whose six-character serial is printed on the bulb.
    fn search_new_lights(
        &self,
        _username: &str,
        _serial: &str,
    ) -> anyhow::Result<Vec<HueBridgeSearchLight>> {
        anyhow::bail!("Hue Bridge serial search is not supported by this transport")
    }

    /// Check whether a Hue V2 device is still owned by the bridge.
    fn device_exists(&self, _username: &str, _v2_device_id: &str) -> anyhow::Result<bool> {
        anyhow::bail!("Hue Bridge device lookup is not supported by this transport")
    }

    /// Remove every V1 light belonging to a Hue V2 light device.
    ///
    /// Hue exposes light deletion only through its V1 API. Implementations
    /// therefore resolve the V2 device to its Zigbee MAC, match that MAC
    /// against V1 `uniqueid` values, delete every exact match, and wait for
    /// the V2 device projection to disappear.
    fn remove_light_device(&self, _username: &str, _v2_device_id: &str) -> anyhow::Result<()> {
        anyhow::bail!("Hue Bridge light removal is not supported by this transport")
    }

    /// Recall a Hue V2 scene.
    fn recall_scene(
        &self,
        _username: &str,
        _scene_id: &str,
        _transition_ms: Option<u32>,
    ) -> anyhow::Result<()> {
        anyhow::bail!("Hue scene recall is not supported by this transport")
    }

    /// Replace the device children of a Hue V2 room.
    ///
    /// Hue room membership is authoritative for grouped-light dispatch, so
    /// logical device moves must update this resource before local topology is
    /// committed.
    fn update_room_children(
        &self,
        _username: &str,
        _room_id: &str,
        _device_ids: &[String],
    ) -> anyhow::Result<()> {
        anyhow::bail!("Hue room membership updates are not supported by this transport")
    }

    /// Drop cached TLS/HTTP connections to free memory.
    ///
    /// Called after bulk discovery is complete to release heap before
    /// the runtime creates its own transport. Default is a no-op;
    /// custom transports can clear their keep-alive connection.
    fn release_connection(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_serial_is_six_hex_characters() {
        assert_eq!(normalize_hue_bridge_serial("e277da").unwrap(), "E277DA");
        assert_eq!(normalize_hue_bridge_serial("e2 77-da").unwrap(), "E277DA");
        assert!(normalize_hue_bridge_serial("E277D").is_err());
        assert!(normalize_hue_bridge_serial("E277D!").is_err());
    }
}
