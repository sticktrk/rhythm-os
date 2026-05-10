//! Hue transport abstraction.
//!
//! Defines the `HueTransport` trait that platform-specific crates implement
//! to provide HTTP communication with the Hue bridge. The trait methods
//! mirror the public HTTP operations required by the integration.

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

    /// Check if a grouped_light (room) has any lights on.
    fn is_grouped_light_on(&self, username: &str, grouped_light_id: &str) -> anyhow::Result<bool>;

    /// Trigger the Hue V2 native identify effect on a single light resource.
    ///
    /// Uses `PUT /clip/v2/resource/light/{id}` with `identify.action = "breathe"`,
    /// which is the canonical mechanism for physical identification on Hue
    /// hardware and does not perturb the on/brightness state.
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

    /// Drop cached TLS/HTTP connections to free memory.
    ///
    /// Called after bulk discovery is complete to release heap before
    /// the runtime creates its own transport. Default is a no-op;
    /// custom transports can clear their keep-alive connection.
    fn release_connection(&self) {}
}
