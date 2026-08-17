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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HueBridgeSearchSensor {
    /// Legacy Hue V1 sensor identifier returned by `/sensors/new`.
    pub legacy_id: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HueBridgeDeviceClass {
    Light,
    Sensor,
}

/// Complete, typed definition used when Rhythm creates or replaces a Hue room.
///
/// `device_ids` are Hue V2 `device` resource IDs. The archetype remains a
/// string because Hue can add archetypes without a Rhythm release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HueRoomDefinition {
    pub name: String,
    pub archetype: String,
    pub device_ids: Vec<String>,
}

/// Identity returned by a successful Hue V2 room creation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HueCreatedRoom {
    pub room_id: String,
}

/// Identity returned by a successful generic Hue V2 resource creation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HueCreatedResource {
    pub resource_id: String,
    pub resource_type: String,
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

    /// Fetch the complete Hue V2 resource inventory in one bridge snapshot.
    ///
    /// Authority acquisition uses this endpoint as its durable pre-mutation
    /// record. The default keeps older test and platform transports source
    /// compatible; production transports should target `/clip/v2/resource`
    /// directly so unknown and newly-added Hue resource types are retained.
    fn get_all_resources(&self, username: &str) -> anyhow::Result<serde_json::Value> {
        self.get_resources(username, "")
    }

    /// Create one Hue V2 resource and return the bridge-confirmed identity.
    ///
    /// This control-plane primitive exists for ownership capture/clear/restore;
    /// ordinary light control should continue to use the typed methods above.
    fn create_resource(
        &self,
        _username: &str,
        _resource_type: &str,
        _body: &serde_json::Value,
    ) -> anyhow::Result<HueCreatedResource> {
        anyhow::bail!("Generic Hue V2 resource creation is not supported by this transport")
    }

    /// Update one Hue V2 control-plane resource.
    ///
    /// Success means the bridge response contained exactly one receipt whose
    /// `rid` and `rtype` match the requested resource. Implementations must
    /// reject an empty, ambiguous, or mismatched success envelope.
    fn update_resource(
        &self,
        _username: &str,
        _resource_type: &str,
        _resource_id: &str,
        _body: &serde_json::Value,
    ) -> anyhow::Result<()> {
        anyhow::bail!("Generic Hue V2 resource updates are not supported by this transport")
    }

    /// Delete one Hue V2 control-plane resource.
    ///
    /// Success has the same exact receipt requirement as [`Self::update_resource`].
    fn delete_resource(
        &self,
        _username: &str,
        _resource_type: &str,
        _resource_id: &str,
    ) -> anyhow::Result<()> {
        anyhow::bail!("Generic Hue V2 resource deletion is not supported by this transport")
    }

    /// Read a Hue V1 API path relative to `/api/{username}`.
    fn get_v1(&self, _username: &str, _path: &str) -> anyhow::Result<serde_json::Value> {
        anyhow::bail!("Generic Hue V1 reads are not supported by this transport")
    }

    /// Create or invoke a Hue V1 API path relative to `/api/{username}`.
    ///
    /// Implementations must reject responses that do not contain a success
    /// receipt for the requested collection.
    fn post_v1(
        &self,
        _username: &str,
        _path: &str,
        _body: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        anyhow::bail!("Generic Hue V1 writes are not supported by this transport")
    }

    /// Update a Hue V1 API path relative to `/api/{username}`.
    ///
    /// Implementations must confirm the exact success address for every
    /// top-level field in the request body.
    fn put_v1(
        &self,
        _username: &str,
        _path: &str,
        _body: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        anyhow::bail!("Generic Hue V1 writes are not supported by this transport")
    }

    /// Delete a Hue V1 API path relative to `/api/{username}`.
    ///
    /// Implementations must confirm the exact deleted resource address.
    fn delete_v1(&self, _username: &str, _path: &str) -> anyhow::Result<serde_json::Value> {
        anyhow::bail!("Generic Hue V1 deletion is not supported by this transport")
    }

    /// Ask a connected Hue Bridge to search its Zigbee network for the bulb
    /// whose six-character serial is printed on the bulb.
    fn search_new_lights(
        &self,
        _username: &str,
        _serial: &str,
    ) -> anyhow::Result<Vec<HueBridgeSearchLight>> {
        anyhow::bail!("Hue Bridge serial search is not supported by this transport")
    }

    /// Ask the Bridge to discover new Zigbee sensors/accessories.
    fn search_new_sensors(&self, _username: &str) -> anyhow::Result<Vec<HueBridgeSearchSensor>> {
        anyhow::bail!("Hue Bridge accessory search is not supported by this transport")
    }

    /// Check whether a Hue V2 device is still owned by the bridge.
    fn device_exists(&self, _username: &str, _v2_device_id: &str) -> anyhow::Result<bool> {
        anyhow::bail!("Hue Bridge device lookup is not supported by this transport")
    }

    /// Remove every V1 resource in the selected device class belonging to one
    /// Hue V2 device.
    ///
    /// Hue exposes light deletion only through its V1 API. Implementations
    /// therefore resolve the V2 device to its Zigbee MAC, match that MAC
    /// against V1 `uniqueid` values, delete every exact match from the owning
    /// V1 collections, and wait for the V2 device projection to disappear.
    fn remove_device(
        &self,
        _username: &str,
        _v2_device_id: &str,
        _device_class: HueBridgeDeviceClass,
    ) -> anyhow::Result<()> {
        anyhow::bail!("Hue Bridge device removal is not supported by this transport")
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

    /// Create a Hue V2 room owned explicitly by Rhythm.
    fn create_room(
        &self,
        _username: &str,
        _definition: &HueRoomDefinition,
    ) -> anyhow::Result<HueCreatedRoom> {
        anyhow::bail!("Hue room creation is not supported by this transport")
    }

    /// Replace the user-visible metadata and children of a Hue V2 room.
    fn update_room(
        &self,
        _username: &str,
        _room_id: &str,
        _definition: &HueRoomDefinition,
    ) -> anyhow::Result<()> {
        anyhow::bail!("Hue room updates are not supported by this transport")
    }

    /// Rename a Hue V2 room without changing its archetype or children.
    fn rename_room(&self, _username: &str, _room_id: &str, _name: &str) -> anyhow::Result<()> {
        anyhow::bail!("Hue room rename is not supported by this transport")
    }

    /// Rename a Hue V2 device without changing any of its services.
    fn rename_device(&self, _username: &str, _device_id: &str, _name: &str) -> anyhow::Result<()> {
        anyhow::bail!("Hue device rename is not supported by this transport")
    }

    /// Delete a Hue V2 room previously recorded as Rhythm-managed.
    fn delete_room(&self, _username: &str, _room_id: &str) -> anyhow::Result<()> {
        anyhow::bail!("Hue room deletion is not supported by this transport")
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
