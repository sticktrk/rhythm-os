//! Serde response structs for all API endpoints.
//!
//! Centralizes JSON shapes so handlers use typed structs instead of
//! `format!()` string concatenation. Field names match the SSE types
//! in `server_event.rs` — one canonical naming convention.

use rhythm_core::runtime::hub_registry::DeviceType;
use serde::Serialize;

// ---------------------------------------------------------------------------
// Room state structs
// ---------------------------------------------------------------------------

/// Core room rhythm state — used by mutation responses and as the base
/// for poll and full-state variants.
#[derive(Clone, Debug, Serialize)]
pub struct RoomRhythmState {
    pub id: String,
    /// Which hub types have lights in this room (e.g. ["hue"], ["hue", "matter"]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hub_types: Vec<String>,
    pub rhythm_enabled: bool,
    pub time_offset: f32,
    pub brightness_offset: f32,
    pub soft_off: bool,
    pub lights_on: bool,
    pub brightness: u8,
    pub kelvin: u16,
}

/// Room state for poll endpoint (`GET /api/rooms/state`).
///
/// Extends `RoomRhythmState` with optional motion fields (using SSE names).
#[derive(Clone, Debug, Serialize)]
pub struct RoomPollState {
    #[serde(flatten)]
    pub rhythm: RoomRhythmState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion_active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion_owned: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning_active: Option<bool>,
}

/// Full room state for `GET /api/state` snapshot.
#[derive(Clone, Debug, Serialize)]
pub struct RoomFullState {
    #[serde(flatten)]
    pub rhythm: RoomRhythmState,
    pub name: String,
    pub grouped_light_id: String,
    pub disabled: bool,
    pub device_ids: Vec<String>,
    pub devices: Vec<TypedDeviceDto>,
}

// ---------------------------------------------------------------------------
// Wrapper responses
// ---------------------------------------------------------------------------

/// Batch room response — `{"rooms": [...]}`.
#[derive(Debug, Serialize)]
pub struct RoomsResponse {
    pub rooms: Vec<RoomRhythmState>,
}

/// Poll response — `{"hub_connected": bool, "rooms": [...]}`.
#[derive(Debug, Serialize)]
pub struct RoomsPollResponse {
    pub hub_connected: bool,
    pub rooms: Vec<RoomPollState>,
}

/// Full state snapshot for `GET /api/state`.
#[derive(Debug, Serialize)]
pub struct StateSnapshot {
    pub version: String,
    pub platform: String,
    pub context: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listen_port: Option<u16>,
    /// Primary hub (backward compat — first connected hub).
    pub hub: HubDto,
    /// All connected hubs (multi-hub support).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hubs: Vec<HubDto>,
    pub config: serde_json::Value,
    pub location: LocationDto,
    pub settings: SettingsDto,
    pub rooms: Vec<RoomFullState>,
    /// Epoch milliseconds of the most recent periodic tick (for client bootstrap).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_tick_epoch_ms: Option<u64>,
}

// ---------------------------------------------------------------------------
// Component DTOs
// ---------------------------------------------------------------------------

/// Hub status in state snapshot.
#[derive(Clone, Debug, Serialize)]
pub struct HubDto {
    #[serde(rename = "type")]
    pub hub_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    pub connected: bool,
}

/// Location in state snapshot.
#[derive(Debug, Serialize)]
pub struct LocationDto {
    pub latitude: Option<f32>,
    pub longitude: Option<f32>,
    pub utc_offset_hours: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone_name: Option<String>,
}

/// Settings in state snapshot and `GET /api/settings`.
#[derive(Debug, Serialize)]
pub struct SettingsDto {
    pub bulb_fade_ms: u16,
    pub rhythm_interval_secs: u64,
    pub default_motion_timeout_secs: u64,
    pub power_save: bool,
    pub soft_off_brightness: u8,
}

/// A typed device entry.
#[derive(Clone, Debug, Serialize)]
pub struct TypedDeviceDto {
    pub id: String,
    #[serde(rename = "type")]
    pub device_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl TypedDeviceDto {
    pub fn new(id: String, dt: &DeviceType) -> Self {
        Self {
            id,
            device_type: Self::type_str(dt).to_string(),
            name: None,
            manufacturer: None,
            model: None,
        }
    }

    pub fn enriched(
        id: String,
        dt: &DeviceType,
        name: Option<String>,
        manufacturer: Option<String>,
        model: Option<String>,
    ) -> Self {
        Self {
            id,
            device_type: Self::type_str(dt).to_string(),
            name,
            manufacturer,
            model,
        }
    }

    fn type_str(dt: &DeviceType) -> &'static str {
        match dt {
            DeviceType::Light => "light",
            DeviceType::Button => "button",
            DeviceType::Motion => "motion",
        }
    }
}

// ---------------------------------------------------------------------------
// Mutation responses
// ---------------------------------------------------------------------------

/// Response for `POST /api/rooms/fix`.
#[derive(Debug, Serialize)]
pub struct FixResponse {
    pub rooms_reset: usize,
    pub rooms: Vec<RoomRhythmState>,
    pub motion_cleared: usize,
    pub motion_rooms: Vec<String>,
}

/// Response for `POST /api/sync`.
#[derive(Debug, Serialize)]
pub struct SyncResponse {
    pub rooms_added: usize,
    pub rooms_updated: usize,
    pub rooms_removed: usize,
    pub devices_synced: usize,
}

/// Response for `PUT /api/hub/credentials`.
#[derive(Debug, Serialize)]
pub struct HubCredentialsResponse {
    pub hub_connected: bool,
}

// ---------------------------------------------------------------------------
// Curve visualization responses
// ---------------------------------------------------------------------------

/// Full curve response for `GET /api/curve` and `POST /api/curve`.
#[derive(Debug, Serialize)]
pub struct CurveResponse {
    pub config: serde_json::Value,
    pub solar: SolarResponse,
    pub curve: rhythm_curve::CurveData,
    pub steps: rhythm_curve::StepSequences,
}

/// Solar times and twilight data.
#[derive(Debug, Serialize)]
pub struct SolarResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sunrise: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sunset: Option<f32>,
    pub solar_noon: f32,
    pub solar_midnight: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub day_length: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub twilight: Option<TwilightResponse>,
}

/// Twilight dawn/dusk phases.
#[derive(Debug, Serialize)]
pub struct TwilightResponse {
    pub dawn: TwilightPhaseResponse,
    pub dusk: TwilightPhaseResponse,
}

/// A single twilight phase (civil/nautical/astronomical).
#[derive(Debug, Serialize)]
pub struct TwilightPhaseResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub civil: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nautical: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub astronomical: Option<f32>,
}

/// Current lighting values for `GET /api/curve/now`.
#[derive(Debug, Serialize)]
pub struct LightingNowResponse {
    pub hour: f32,
    pub brightness: u8,
    pub kelvin: u16,
    pub mireds: u16,
    pub rgb: rhythm_curve::Rgb,
    pub xy: rhythm_curve::XyColor,
    pub solar_time: f32,
    pub sun_position: f32,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn sample_rhythm_state() -> RoomRhythmState {
        RoomRhythmState {
            id: "room1".into(),
            hub_types: vec!["hue".into()],
            rhythm_enabled: true,
            time_offset: 5.0,
            brightness_offset: -10.0,
            soft_off: false,
            lights_on: true,
            brightness: 80,
            kelvin: 4000,
        }
    }

    // ---- RoomRhythmState ----

    #[test]
    fn room_rhythm_state_serializes_all_fields() {
        let state = sample_rhythm_state();
        let json: Value = serde_json::to_value(&state).unwrap();
        assert_eq!(json["id"], "room1");
        assert_eq!(json["rhythm_enabled"], true);
        assert_eq!(json["time_offset"], 5.0);
        assert_eq!(json["brightness_offset"], -10.0);
        assert_eq!(json["soft_off"], false);
        assert_eq!(json["lights_on"], true);
        assert_eq!(json["brightness"], 80);
        assert_eq!(json["kelvin"], 4000);
        // No status wrapper
        assert!(json.get("status").is_none());
    }

    // ---- RoomPollState ----

    #[test]
    fn room_poll_state_flattens_rhythm_fields() {
        let poll = RoomPollState {
            rhythm: sample_rhythm_state(),
            motion_active: None,
            motion_owned: None,
            remaining_secs: None,
            timeout_secs: None,
            warning_active: None,
        };
        let json: Value = serde_json::to_value(&poll).unwrap();
        // Flattened rhythm fields appear at top level
        assert_eq!(json["id"], "room1");
        assert_eq!(json["brightness"], 80);
        // Motion fields omitted when None
        assert!(json.get("motion_active").is_none());
        assert!(json.get("motion_owned").is_none());
        assert!(json.get("remaining_secs").is_none());
        assert!(json.get("timeout_secs").is_none());
        assert!(json.get("warning_active").is_none());
    }

    #[test]
    fn room_poll_state_includes_motion_when_present() {
        let poll = RoomPollState {
            rhythm: sample_rhythm_state(),
            motion_active: Some(true),
            motion_owned: Some(false),
            remaining_secs: Some(120),
            timeout_secs: Some(300),
            warning_active: Some(true),
        };
        let json: Value = serde_json::to_value(&poll).unwrap();
        assert_eq!(json["motion_active"], true);
        assert_eq!(json["motion_owned"], false);
        assert_eq!(json["remaining_secs"], 120);
        assert_eq!(json["timeout_secs"], 300);
        assert_eq!(json["warning_active"], true);
        // SSE-aligned names — old names must NOT appear
        assert!(json.get("motion_remaining").is_none());
        assert!(json.get("motion_timeout").is_none());
        assert!(json.get("motion_warning").is_none());
    }

    // ---- RoomFullState ----

    #[test]
    fn room_full_state_flattens_with_extra_fields() {
        let full = RoomFullState {
            rhythm: sample_rhythm_state(),
            name: "Living Room".into(),
            grouped_light_id: "gl_abc".into(),
            disabled: false,
            device_ids: vec!["d1".into(), "d2".into()],
            devices: vec![
                TypedDeviceDto::new("d1".into(), &DeviceType::Light),
                TypedDeviceDto::new("d2".into(), &DeviceType::Button),
            ],
        };
        let json: Value = serde_json::to_value(&full).unwrap();
        // Flattened from rhythm
        assert_eq!(json["id"], "room1");
        assert_eq!(json["kelvin"], 4000);
        // Full-state only fields
        assert_eq!(json["name"], "Living Room");
        assert_eq!(json["grouped_light_id"], "gl_abc");
        assert_eq!(json["disabled"], false);
        assert_eq!(json["device_ids"], serde_json::json!(["d1", "d2"]));
        assert_eq!(json["devices"][0]["id"], "d1");
        assert_eq!(json["devices"][0]["type"], "light");
        assert_eq!(json["devices"][1]["type"], "button");
    }

    // ---- RoomsResponse ----

    #[test]
    fn rooms_response_wraps_in_rooms_array() {
        let resp = RoomsResponse {
            rooms: vec![sample_rhythm_state()],
        };
        let json: Value = serde_json::to_value(&resp).unwrap();
        assert!(json["rooms"].is_array());
        assert_eq!(json["rooms"].as_array().unwrap().len(), 1);
        assert_eq!(json["rooms"][0]["id"], "room1");
        // No status wrapper
        assert!(json.get("status").is_none());
    }

    #[test]
    fn rooms_response_empty_array() {
        let resp = RoomsResponse { rooms: vec![] };
        let json: Value = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["rooms"].as_array().unwrap().len(), 0);
    }

    // ---- RoomsPollResponse ----

    #[test]
    fn rooms_poll_response_shape() {
        let resp = RoomsPollResponse {
            hub_connected: true,
            rooms: vec![RoomPollState {
                rhythm: sample_rhythm_state(),
                motion_active: Some(true),
                motion_owned: Some(true),
                remaining_secs: Some(60),
                timeout_secs: Some(300),
                warning_active: Some(false),
            }],
        };
        let json: Value = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["hub_connected"], true);
        assert_eq!(json["rooms"][0]["id"], "room1");
        assert_eq!(json["rooms"][0]["remaining_secs"], 60);
        // No status wrapper
        assert!(json.get("status").is_none());
    }

    // ---- SettingsDto ----

    #[test]
    fn settings_dto_serializes() {
        let dto = SettingsDto {
            bulb_fade_ms: 500,
            rhythm_interval_secs: 60,
            default_motion_timeout_secs: 300,
            power_save: true,
            soft_off_brightness: 5,
        };
        let json: Value = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["bulb_fade_ms"], 500);
        assert_eq!(json["rhythm_interval_secs"], 60);
        assert_eq!(json["default_motion_timeout_secs"], 300);
        assert_eq!(json["power_save"], true);
        assert_eq!(json["soft_off_brightness"], 5);
    }

    // ---- HubDto ----

    #[test]
    fn hub_dto_type_field_renamed() {
        let hub = HubDto {
            hub_type: "hue".into(),
            address: Some("192.168.1.100".into()),
            connected: true,
        };
        let json: Value = serde_json::to_value(&hub).unwrap();
        // Serialized as "type", not "hub_type"
        assert_eq!(json["type"], "hue");
        assert!(json.get("hub_type").is_none());
        assert_eq!(json["address"], "192.168.1.100");
        assert_eq!(json["connected"], true);
    }

    #[test]
    fn hub_dto_address_omitted_when_none() {
        let hub = HubDto {
            hub_type: "none".into(),
            address: None,
            connected: false,
        };
        let json: Value = serde_json::to_value(&hub).unwrap();
        assert!(json.get("address").is_none());
        assert_eq!(json["type"], "none");
        assert_eq!(json["connected"], false);
    }

    // ---- LocationDto ----

    #[test]
    fn location_dto_with_timezone() {
        let loc = LocationDto {
            latitude: Some(35.6),
            longitude: Some(-97.5),
            utc_offset_hours: -6.0,
            timezone_name: Some("America/Chicago".into()),
        };
        let json: Value = serde_json::to_value(&loc).unwrap();
        assert_eq!(json["latitude"], 35.6_f32 as f64);
        assert_eq!(json["longitude"], -97.5_f32 as f64);
        assert_eq!(json["utc_offset_hours"], -6.0_f32 as f64);
        assert_eq!(json["timezone_name"], "America/Chicago");
    }

    #[test]
    fn location_dto_null_optionals() {
        let loc = LocationDto {
            latitude: None,
            longitude: None,
            utc_offset_hours: 0.0,
            timezone_name: None,
        };
        let json: Value = serde_json::to_value(&loc).unwrap();
        assert!(json["latitude"].is_null());
        assert!(json["longitude"].is_null());
        assert!(json.get("timezone_name").is_none());
    }

    // ---- TypedDeviceDto ----

    #[test]
    fn typed_device_dto_type_field_renamed() {
        let d = TypedDeviceDto::new("dev1".into(), &DeviceType::Motion);
        let json: Value = serde_json::to_value(&d).unwrap();
        assert_eq!(json["id"], "dev1");
        assert_eq!(json["type"], "motion");
        assert!(json.get("device_type").is_none());
    }

    // ---- FixResponse ----

    #[test]
    fn fix_response_rooms_are_objects() {
        let resp = FixResponse {
            rooms_reset: 1,
            rooms: vec![sample_rhythm_state()],
            motion_cleared: 0,
            motion_rooms: vec![],
        };
        let json: Value = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["rooms_reset"], 1);
        // rooms contains objects with id, not bare ID strings
        assert!(json["rooms"][0].is_object());
        assert_eq!(json["rooms"][0]["id"], "room1");
        assert_eq!(json["rooms"][0]["brightness"], 80);
        assert_eq!(json["motion_cleared"], 0);
        assert!(json["motion_rooms"].as_array().unwrap().is_empty());
        // No status wrapper
        assert!(json.get("status").is_none());
        // No room_states key (old shape)
        assert!(json.get("room_states").is_none());
    }

    // ---- SyncResponse ----

    #[test]
    fn sync_response_no_status_wrapper() {
        let resp = SyncResponse {
            rooms_added: 3,
            rooms_updated: 1,
            rooms_removed: 0,
            devices_synced: 8,
        };
        let json: Value = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["rooms_added"], 3);
        assert_eq!(json["rooms_updated"], 1);
        assert_eq!(json["rooms_removed"], 0);
        assert_eq!(json["devices_synced"], 8);
        assert!(json.get("status").is_none());
    }

    // ---- HubCredentialsResponse ----

    #[test]
    fn hub_credentials_response_no_status_wrapper() {
        let resp = HubCredentialsResponse {
            hub_connected: true,
        };
        let json: Value = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["hub_connected"], true);
        assert!(json.get("status").is_none());
    }

    // ---- StateSnapshot ----

    #[test]
    fn state_snapshot_omits_listen_port_when_none() {
        let snap = StateSnapshot {
            version: "1.0.0".into(),
            platform: "desktop".into(),
            context: "server".into(),
            listen_port: None,
            hub: HubDto {
                hub_type: "none".into(),
                address: None,
                connected: false,
            },
            hubs: vec![],
            config: serde_json::json!({}),
            location: LocationDto {
                latitude: None,
                longitude: None,
                utc_offset_hours: 0.0,
                timezone_name: None,
            },
            settings: SettingsDto {
                bulb_fade_ms: 500,
                rhythm_interval_secs: 60,
                default_motion_timeout_secs: 300,
                power_save: false,
                soft_off_brightness: 1,
            },
            rooms: vec![],
            last_tick_epoch_ms: None,
        };
        let json: Value = serde_json::to_value(&snap).unwrap();
        assert!(json.get("listen_port").is_none());
        // hubs omitted when empty
        assert!(json.get("hubs").is_none());
        // last_tick_epoch_ms omitted when None
        assert!(json.get("last_tick_epoch_ms").is_none());
        assert_eq!(json["version"], "1.0.0");
        assert_eq!(json["platform"], "desktop");
        assert_eq!(json["context"], "server");
        assert!(json["rooms"].as_array().unwrap().is_empty());
    }

    #[test]
    fn state_snapshot_includes_listen_port_when_set() {
        let snap = StateSnapshot {
            version: "1.0.0".into(),
            platform: "desktop".into(),
            context: "ha_addon".into(),
            listen_port: Some(8099),
            hub: HubDto {
                hub_type: "hue".into(),
                address: Some("192.168.1.2".into()),
                connected: true,
            },
            hubs: vec![HubDto {
                hub_type: "hue".into(),
                address: Some("192.168.1.2".into()),
                connected: true,
            }],
            config: serde_json::json!({"min_brightness": 1}),
            location: LocationDto {
                latitude: Some(35.0),
                longitude: Some(-97.0),
                utc_offset_hours: -6.0,
                timezone_name: Some("America/Chicago".into()),
            },
            settings: SettingsDto {
                bulb_fade_ms: 500,
                rhythm_interval_secs: 60,
                default_motion_timeout_secs: 300,
                power_save: false,
                soft_off_brightness: 1,
            },
            rooms: vec![RoomFullState {
                rhythm: sample_rhythm_state(),
                name: "Office".into(),
                grouped_light_id: "gl1".into(),
                disabled: false,
                device_ids: vec![],
                devices: vec![],
            }],
            last_tick_epoch_ms: Some(1700000000000),
        };
        let json: Value = serde_json::to_value(&snap).unwrap();
        assert_eq!(json["listen_port"], 8099);
        assert_eq!(json["rooms"].as_array().unwrap().len(), 1);
        assert_eq!(json["rooms"][0]["name"], "Office");
        assert_eq!(json["hub"]["type"], "hue");
        assert_eq!(json["last_tick_epoch_ms"], 1700000000000u64);
    }
}
