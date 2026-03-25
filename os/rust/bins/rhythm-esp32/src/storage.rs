//! NVS storage for persistent configuration.
//!
//! Stores curve configuration, hub credentials, room state, and location
//! in NVS. Hub-specific persistence (e.g., Hue registry snapshots) lives
//! in the respective hub modules.

use std::collections::HashMap;

use anyhow::Result;
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use rhythm_core::config::CurveConfig;
use rhythm_core::room::{Room, RoomManager, RoomSource};

use rhythm_os::hub::{HubCredentials, HubType};
use rhythm_os::state::SharedState;

// ============================================================================
// Compact NVS Types — short field names for embedded storage
// ============================================================================

#[derive(Serialize, Deserialize)]
struct NvsRoomManager {
    #[serde(rename = "r")]
    rooms: HashMap<String, NvsRoom>,
}

#[derive(Serialize, Deserialize)]
struct NvsRoom {
    #[serde(rename = "i")]
    id: String,
    #[serde(rename = "n")]
    name: String,
    #[serde(rename = "s", default, skip_serializing_if = "nvs_source_is_default")]
    source: NvsRoomSource,
    #[serde(rename = "re")]
    rhythm_enabled: bool,
    #[serde(rename = "d", default, skip_serializing_if = "is_false")]
    disabled: bool,
    #[serde(rename = "to", default, skip_serializing_if = "is_zero_f32")]
    time_offset_minutes: f32,
    #[serde(rename = "bo", default, skip_serializing_if = "is_zero_f32")]
    brightness_offset: f32,
    #[serde(rename = "cc", default, skip_serializing_if = "Option::is_none")]
    curve_config: Option<NvsCurveConfig>,
    #[serde(rename = "so", default, skip_serializing_if = "is_false")]
    soft_off: bool,
}

#[derive(Serialize, Deserialize, Default, PartialEq)]
enum NvsRoomSource {
    #[default]
    #[serde(rename = "U")]
    Unknown,
    #[serde(rename = "H")]
    Hue,
    #[serde(rename = "HA")]
    HomeAssistant,
    #[serde(rename = "E")]
    Esp32,
    #[serde(untagged)]
    Other(String),
}

#[derive(Serialize, Deserialize)]
struct NvsCurveConfig {
    #[serde(rename = "ct0")]
    min_color_temp: u16,
    #[serde(rename = "ct1")]
    max_color_temp: u16,
    #[serde(rename = "b0")]
    min_brightness: u8,
    #[serde(rename = "b1")]
    max_brightness: u8,
    #[serde(rename = "wlb")]
    width_left_bri: f32,
    #[serde(rename = "wrb")]
    width_right_bri: f32,
    #[serde(rename = "wlc")]
    width_left_cct: f32,
    #[serde(rename = "wrc")]
    width_right_cct: f32,
    #[serde(rename = "sp")]
    shape_p: f32,
    #[serde(rename = "ds")]
    max_dim_steps: u8,
}

fn is_false(v: &bool) -> bool { !v }
fn is_zero_f32(v: &f32) -> bool { *v == 0.0 }
fn nvs_source_is_default(v: &NvsRoomSource) -> bool { *v == NvsRoomSource::Unknown }

// --- Conversions ---

impl From<&Room> for NvsRoom {
    fn from(r: &Room) -> Self {
        Self {
            id: r.id.clone(),
            name: r.name.clone(),
            source: NvsRoomSource::from(&r.source),
            rhythm_enabled: r.rhythm_enabled,
            disabled: r.disabled,
            time_offset_minutes: r.time_offset_minutes,
            brightness_offset: r.brightness_offset,
            curve_config: r.curve_config.as_ref().map(NvsCurveConfig::from),
            soft_off: r.soft_off,
        }
    }
}

impl From<NvsRoom> for Room {
    fn from(r: NvsRoom) -> Self {
        Self {
            id: r.id,
            name: r.name,
            source: RoomSource::from(r.source),
            rhythm_enabled: r.rhythm_enabled,
            disabled: r.disabled,
            time_offset_minutes: r.time_offset_minutes,
            brightness_offset: r.brightness_offset,
            curve_config: r.curve_config.map(CurveConfig::from),
            soft_off: r.soft_off,
        }
    }
}

impl From<&RoomSource> for NvsRoomSource {
    fn from(s: &RoomSource) -> Self {
        match s {
            RoomSource::Unknown => Self::Unknown,
            RoomSource::Hue => Self::Hue,
            RoomSource::HomeAssistant => Self::HomeAssistant,
            RoomSource::Esp32 => Self::Esp32,
            RoomSource::Other(s) => Self::Other(s.clone()),
        }
    }
}

impl From<NvsRoomSource> for RoomSource {
    fn from(s: NvsRoomSource) -> Self {
        match s {
            NvsRoomSource::Unknown => Self::Unknown,
            NvsRoomSource::Hue => Self::Hue,
            NvsRoomSource::HomeAssistant => Self::HomeAssistant,
            NvsRoomSource::Esp32 => Self::Esp32,
            NvsRoomSource::Other(s) => Self::Other(s),
        }
    }
}

impl From<&CurveConfig> for NvsCurveConfig {
    fn from(c: &CurveConfig) -> Self {
        Self {
            min_color_temp: c.min_color_temp,
            max_color_temp: c.max_color_temp,
            min_brightness: c.min_brightness,
            max_brightness: c.max_brightness,
            width_left_bri: c.width_left_bri,
            width_right_bri: c.width_right_bri,
            width_left_cct: c.width_left_cct,
            width_right_cct: c.width_right_cct,
            shape_p: c.shape_p,
            max_dim_steps: c.max_dim_steps,
        }
    }
}

impl From<NvsCurveConfig> for CurveConfig {
    fn from(c: NvsCurveConfig) -> Self {
        Self {
            min_color_temp: c.min_color_temp,
            max_color_temp: c.max_color_temp,
            min_brightness: c.min_brightness,
            max_brightness: c.max_brightness,
            width_left_bri: c.width_left_bri,
            width_right_bri: c.width_right_bri,
            width_left_cct: c.width_left_cct,
            width_right_cct: c.width_right_cct,
            shape_p: c.shape_p,
            max_dim_steps: c.max_dim_steps,
        }
    }
}

impl From<&RoomManager> for NvsRoomManager {
    fn from(rm: &RoomManager) -> Self {
        Self {
            rooms: rm.iter().map(|r| (r.id.clone(), NvsRoom::from(r))).collect(),
        }
    }
}

impl From<NvsRoomManager> for RoomManager {
    fn from(nvs: NvsRoomManager) -> Self {
        let mut rm = RoomManager::new();
        for (_id, nvs_room) in nvs.rooms {
            rm.add_room(Room::from(nvs_room));
        }
        rm
    }
}

// ============================================================================
// Room State Storage (standalone — not tied to AppState)
// ============================================================================

/// Load persisted room state from NVS.
///
/// Tries compact format first, falls back to verbose (pre-migration) format.
pub fn load_rooms(nvs: &EspDefaultNvsPartition) -> Result<RoomManager> {
    let nvs = EspNvs::new(nvs.clone(), NVS_NAMESPACE, true)?;
    let mut buf = [0u8; 4096];
    match nvs.get_str(KEY_ROOMS, &mut buf) {
        Ok(Some(json)) => {
            // Try compact format first
            if let Ok(nvs_rm) = serde_json::from_str::<NvsRoomManager>(json) {
                let rooms = RoomManager::from(nvs_rm);
                info!("Loaded {} rooms from NVS (compact)", rooms.len());
                return Ok(rooms);
            }
            // Fall back to verbose format (migration)
            match serde_json::from_str::<RoomManager>(json) {
                Ok(rooms) => {
                    info!("Loaded {} rooms from NVS (verbose, will compact on next save)", rooms.len());
                    return Ok(rooms);
                }
                Err(e) => warn!("Failed to parse rooms JSON from NVS: {}", e),
            }
        }
        Ok(None) => {}
        Err(e) => warn!("Failed to read rooms from NVS (buffer too small?): {}", e),
    }
    Ok(RoomManager::new())
}

/// Save room state to NVS using compact format.
pub fn save_rooms(nvs: &EspDefaultNvsPartition, rooms: &RoomManager) -> Result<()> {
    let nvs = EspNvs::new(nvs.clone(), NVS_NAMESPACE, true)?;
    let compact = NvsRoomManager::from(rooms);
    if let Ok(json) = serde_json::to_string(&compact) {
        info!("Saving {} rooms to NVS ({} bytes)", rooms.len(), json.len());
        nvs.set_str(KEY_ROOMS, &json)?;
    }
    Ok(())
}

pub const NVS_NAMESPACE: &str = "rhythm";

// NVS keys (max 15 chars)
const KEY_MIN_BRI: &str = "min_bri";
const KEY_MAX_BRI: &str = "max_bri";
const KEY_MIN_CCT: &str = "min_cct";
const KEY_MAX_CCT: &str = "max_cct";
const KEY_SOLAR_NOON: &str = "solar_noon";
const KEY_UTC_OFFSET: &str = "utc_offset";
const KEY_HUB_TYPE: &str = "hub_type";
const KEY_HUE_IP: &str = "hue_ip";
const KEY_HUE_USER: &str = "hue_user";
/// Room state stored as serialized RoomManager (replaces old separate keys).
const KEY_ROOMS: &str = "rooms";
const KEY_LATITUDE: &str = "latitude";
const KEY_LONGITUDE: &str = "longitude";
pub const KEY_HUE_REG: &str = "hue_reg";
const KEY_WIFI_SSID: &str = "wifi_ssid";
const KEY_WIFI_PASS: &str = "wifi_pass";
const KEY_BULB_FADE_MS: &str = "bulb_fade_ms";
const KEY_RHYTHM_INTV: &str = "rhythm_intv";
const KEY_DFL_MOT_TOUT: &str = "dfl_mot_tout";
const KEY_PWR_SAVE: &str = "pwr_save";
const KEY_SOB: &str = "soft_off_bri";
const KEY_TZ_NAME: &str = "tz_name";
const KEY_CRASH_CNT: &str = "crash_cnt";
const KEY_LAST_RST_RSN: &str = "last_rst_rsn";
const KEY_LAST_CRASH_TS: &str = "last_crash_ts";
const KEY_LAST_PANIC: &str = "last_panic";

/// Load configuration from NVS.
pub fn load_config(nvs: &EspDefaultNvsPartition, state: &SharedState) -> Result<()> {
    let nvs = EspNvs::new(nvs.clone(), NVS_NAMESPACE, true)?;

    let mut state = state
        .lock()
        .map_err(|_| anyhow::anyhow!("Failed to lock state"))?;

    // Load curve config values
    if let Ok(Some(val)) = nvs.get_u8(KEY_MIN_BRI) {
        state.config.min_brightness = val;
        info!("Loaded min_brightness: {}", val);
    }

    if let Ok(Some(val)) = nvs.get_u8(KEY_MAX_BRI) {
        state.config.max_brightness = val;
        info!("Loaded max_brightness: {}", val);
    }

    if let Ok(Some(val)) = nvs.get_u16(KEY_MIN_CCT) {
        state.config.min_color_temp = val;
        info!("Loaded min_color_temp: {}", val);
    }

    if let Ok(Some(val)) = nvs.get_u16(KEY_MAX_CCT) {
        state.config.max_color_temp = val;
        info!("Loaded max_color_temp: {}", val);
    }

    // Load solar noon (stored as i16 representing hour * 100)
    if let Ok(Some(val)) = nvs.get_i16(KEY_SOLAR_NOON) {
        state.runtime_config.solar_noon_hour = val as f32 / 100.0;
        info!("Loaded solar_noon_hour: {}", state.runtime_config.solar_noon_hour);
    }

    // Load UTC offset (stored as i16 representing hours * 100)
    if let Ok(Some(val)) = nvs.get_i16(KEY_UTC_OFFSET) {
        state.utc_offset_hours = val as f32 / 100.0;
        info!("Loaded utc_offset_hours: {}", state.utc_offset_hours);
    }

    // Load hub credentials (hub_type key dispatches to per-hub keys)
    let creds = load_hub_credentials_inner(&nvs);
    if creds.is_configured() {
        if let Some(key) = creds.hub_key() {
            state.hub_credentials.insert(key, creds);
        }
    }

    // Load location (stored as i32 * 10000 for precision)
    if let Ok(Some(val)) = nvs.get_i32(KEY_LATITUDE) {
        state.latitude = Some(val as f32 / 10000.0);
        info!("Loaded latitude: {:.4}", state.latitude.unwrap());
    }

    if let Ok(Some(val)) = nvs.get_i32(KEY_LONGITUDE) {
        state.longitude = Some(val as f32 / 10000.0);
        info!("Loaded longitude: {:.4}", state.longitude.unwrap());
    }

    // Load global settings
    if let Ok(Some(val)) = nvs.get_u16(KEY_BULB_FADE_MS) {
        state.bulb_fade_ms = val;
        state.bulb_fade_atomic.store(val, std::sync::atomic::Ordering::Relaxed);
        info!("Loaded bulb_fade_ms: {}", val);
    }

    if let Ok(Some(val)) = nvs.get_u16(KEY_RHYTHM_INTV) {
        state.runtime_config.update_interval_secs = val as u64;
        info!("Loaded rhythm_interval_secs: {}", val);
    }

    if let Ok(Some(val)) = nvs.get_u16(KEY_DFL_MOT_TOUT) {
        state.default_motion_timeout_secs = val as u64;
        info!("Loaded default_motion_timeout_secs: {}", val);
    }

    if let Ok(Some(val)) = nvs.get_u8(KEY_PWR_SAVE) {
        state.power_save = val != 0;
        info!("Loaded power_save: {}", state.power_save);
    }

    if let Ok(Some(val)) = nvs.get_u8(KEY_SOB) {
        state.soft_off_brightness = val.clamp(1, 100);
        info!("Loaded soft_off_brightness: {}", state.soft_off_brightness);
    }

    Ok(())
}

/// Save configuration to NVS.
pub fn save_config(nvs: &EspDefaultNvsPartition, state: &SharedState) -> Result<()> {
    let nvs = EspNvs::new(nvs.clone(), NVS_NAMESPACE, true)?;

    let state = state
        .lock()
        .map_err(|_| anyhow::anyhow!("Failed to lock state"))?;

    // Save curve config values
    nvs.set_u8(KEY_MIN_BRI, state.config.min_brightness)?;
    nvs.set_u8(KEY_MAX_BRI, state.config.max_brightness)?;
    nvs.set_u16(KEY_MIN_CCT, state.config.min_color_temp)?;
    nvs.set_u16(KEY_MAX_CCT, state.config.max_color_temp)?;

    // Save solar noon (stored as i16 representing hour * 100)
    let solar_noon_encoded = (state.runtime_config.solar_noon_hour * 100.0) as i16;
    nvs.set_i16(KEY_SOLAR_NOON, solar_noon_encoded)?;

    // Save UTC offset (stored as i16 representing hours * 100)
    let utc_offset_encoded = (state.utc_offset_hours * 100.0) as i16;
    nvs.set_i16(KEY_UTC_OFFSET, utc_offset_encoded)?;

    // Save hub credentials (ESP32: single hub, save first configured)
    if let Some(creds) = state.hub_credentials.values().next() {
        save_hub_credentials_inner(&nvs, creds)?;
    }

    // Save location (stored as i32 * 10000 for precision)
    if let Some(lat) = state.latitude {
        nvs.set_i32(KEY_LATITUDE, (lat * 10000.0) as i32)?;
    }
    if let Some(lon) = state.longitude {
        nvs.set_i32(KEY_LONGITUDE, (lon * 10000.0) as i32)?;
    }

    info!("Configuration saved to NVS");
    Ok(())
}

/// Save global settings to NVS.
pub fn save_settings(nvs: &EspDefaultNvsPartition, state: &SharedState) -> Result<()> {
    let nvs = EspNvs::new(nvs.clone(), NVS_NAMESPACE, true)?;

    let state = state
        .lock()
        .map_err(|_| anyhow::anyhow!("Failed to lock state"))?;

    nvs.set_u16(KEY_BULB_FADE_MS, state.bulb_fade_ms)?;
    nvs.set_u16(KEY_RHYTHM_INTV, state.runtime_config.update_interval_secs as u16)?;
    nvs.set_u16(KEY_DFL_MOT_TOUT, state.default_motion_timeout_secs as u16)?;
    nvs.set_u8(KEY_PWR_SAVE, if state.power_save { 1 } else { 0 })?;
    nvs.set_u8(KEY_SOB, state.soft_off_brightness)?;

    info!("Settings saved to NVS");
    Ok(())
}

/// Clear all stored configuration from NVS.
pub fn clear_config(nvs: &EspDefaultNvsPartition) -> Result<()> {
    let nvs = EspNvs::new(nvs.clone(), NVS_NAMESPACE, true)?;

    // Remove all keys
    let _ = nvs.remove(KEY_MIN_BRI);
    let _ = nvs.remove(KEY_MAX_BRI);
    let _ = nvs.remove(KEY_MIN_CCT);
    let _ = nvs.remove(KEY_MAX_CCT);
    let _ = nvs.remove(KEY_SOLAR_NOON);
    let _ = nvs.remove(KEY_UTC_OFFSET);
    let _ = nvs.remove(KEY_HUB_TYPE);
    let _ = nvs.remove(KEY_HUE_IP);
    let _ = nvs.remove(KEY_HUE_USER);
    let _ = nvs.remove(KEY_ROOMS);
    let _ = nvs.remove(KEY_LATITUDE);
    let _ = nvs.remove(KEY_LONGITUDE);
    let _ = nvs.remove(KEY_TZ_NAME);
    let _ = nvs.remove(KEY_WIFI_SSID);
    let _ = nvs.remove(KEY_WIFI_PASS);
    let _ = nvs.remove(KEY_BULB_FADE_MS);
    let _ = nvs.remove(KEY_RHYTHM_INTV);
    let _ = nvs.remove(KEY_DFL_MOT_TOUT);
    let _ = nvs.remove(KEY_PWR_SAVE);

    info!("Configuration cleared from NVS");
    Ok(())
}

// ============================================================================
// Hub Credential Storage
// ============================================================================

/// Inner implementation that works with an already-opened NVS handle.
fn load_hub_credentials_inner(nvs: &EspNvs<NvsDefault>) -> HubCredentials {
    // Try hub_type key first
    let mut type_buf = [0u8; 16];
    let hub_type = nvs
        .get_str(KEY_HUB_TYPE, &mut type_buf)
        .ok()
        .flatten()
        .and_then(|s| HubType::parse(s));

    match hub_type {
        Some(ref ht) if ht.as_str() == HubType::HUE => load_hue_credentials(nvs),
        Some(_) => HubCredentials::default(),
        None => {
            // Backwards compatibility: check if Hue keys exist without hub_type
            let creds = load_hue_credentials(nvs);
            if creds.is_configured() {
                info!("Found legacy Hue credentials (no hub_type key)");
            }
            creds
        }
    }
}

/// Load Hue-specific credentials from NVS.
fn load_hue_credentials(nvs: &EspNvs<NvsDefault>) -> HubCredentials {
    let mut ip_buf = [0u8; 64];
    let mut user_buf = [0u8; 128];

    let ip = nvs.get_str(KEY_HUE_IP, &mut ip_buf).ok().flatten().map(|s| s.to_string());
    let user = nvs.get_str(KEY_HUE_USER, &mut user_buf).ok().flatten().map(|s| s.to_string());

    match (ip, user) {
        (Some(bridge_ip), Some(username)) => {
            info!("Loaded Hue credentials: bridge_ip={}", bridge_ip);
            rhythm_hue::provider::hue_credentials(&bridge_ip, &username)
        }
        _ => HubCredentials::default(),
    }
}

/// Save hub credentials to NVS.
///
/// Writes the `hub_type` key and the appropriate per-hub credential keys.
pub fn save_hub_credentials(nvs: &EspDefaultNvsPartition, credentials: &HubCredentials) -> Result<()> {
    let nvs = EspNvs::new(nvs.clone(), NVS_NAMESPACE, true)?;
    save_hub_credentials_inner(&nvs, credentials)
}

/// Inner implementation that works with an already-opened NVS handle.
fn save_hub_credentials_inner(nvs: &EspNvs<NvsDefault>, credentials: &HubCredentials) -> Result<()> {
    if !credentials.is_configured() {
        let _ = nvs.remove(KEY_HUB_TYPE);
        let _ = nvs.remove(KEY_HUE_IP);
        let _ = nvs.remove(KEY_HUE_USER);
    } else if credentials.hub_type.as_ref().map(|t| t.as_str()) == Some(HubType::HUE) {
        nvs.set_str(KEY_HUB_TYPE, HubType::HUE)?;
        nvs.set_str(KEY_HUE_IP, &credentials.address)?;
        if let Some(username) = rhythm_hue::provider::hue_username(credentials) {
            nvs.set_str(KEY_HUE_USER, username)?;
        }
        info!("Saved Hue credentials to NVS");
    }
    Ok(())
}

// ============================================================================
// WiFi Credential Storage
// ============================================================================

/// Load WiFi credentials from NVS.
pub fn load_wifi_credentials(nvs: &EspDefaultNvsPartition) -> Option<(String, String)> {
    let nvs = EspNvs::new(nvs.clone(), NVS_NAMESPACE, true).ok()?;
    let mut ssid_buf = [0u8; 64];
    let mut pass_buf = [0u8; 128];

    let ssid = nvs.get_str(KEY_WIFI_SSID, &mut ssid_buf).ok()?.map(|s| s.to_string())?;
    let pass = nvs.get_str(KEY_WIFI_PASS, &mut pass_buf).ok()?.map(|s| s.to_string())?;

    info!("Loaded WiFi credentials from NVS: ssid={}", ssid);
    Some((ssid, pass))
}

/// Save WiFi credentials to NVS.
pub fn save_wifi_credentials(nvs: &EspDefaultNvsPartition, ssid: &str, password: &str) -> Result<()> {
    let nvs = EspNvs::new(nvs.clone(), NVS_NAMESPACE, true)?;
    nvs.set_str(KEY_WIFI_SSID, ssid)?;
    nvs.set_str(KEY_WIFI_PASS, password)?;
    info!("Saved WiFi credentials to NVS: ssid={}", ssid);
    Ok(())
}

/// Clear WiFi credentials from NVS.
pub fn clear_wifi_credentials(nvs: &EspDefaultNvsPartition) -> Result<()> {
    let nvs = EspNvs::new(nvs.clone(), NVS_NAMESPACE, true)?;
    let _ = nvs.remove(KEY_WIFI_SSID);
    let _ = nvs.remove(KEY_WIFI_PASS);
    info!("WiFi credentials cleared from NVS");
    Ok(())
}

// ============================================================================
// Location Storage
// ============================================================================

// ============================================================================
// Crash Info Storage
// ============================================================================

/// Crash data persisted across reboots in NVS.
pub struct CrashInfo {
    pub crash_count: u32,
    pub last_reset_reason: u8,
    pub last_crash_ts: u32,
    pub last_panic: Option<String>,
}

/// Save crash info to NVS (called from panic hook or boot detection).
///
/// The `reason` field uses 0xFF as a sentinel when set by the panic hook
/// (real reason not yet known). Boot detection overwrites it with the actual
/// `esp_reset_reason()` value.
pub fn save_crash_info(nvs: &EspDefaultNvsPartition, reason: u8, panic_msg: Option<&str>) -> Result<()> {
    let nvs = EspNvs::new(nvs.clone(), NVS_NAMESPACE, true)?;

    // Increment crash counter
    let count = nvs.get_u32(KEY_CRASH_CNT).ok().flatten().unwrap_or(0) + 1;
    nvs.set_u32(KEY_CRASH_CNT, count)?;

    nvs.set_u8(KEY_LAST_RST_RSN, reason)?;

    // Timestamp: current epoch or 0 if NTP not synced
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0);
    nvs.set_u32(KEY_LAST_CRASH_TS, ts)?;

    if let Some(msg) = panic_msg {
        // Truncate to 200 chars to fit NVS string limits
        let truncated = if msg.len() > 200 { &msg[..200] } else { msg };
        nvs.set_str(KEY_LAST_PANIC, truncated)?;
    }

    Ok(())
}

/// Load crash info from NVS. Returns None if no crashes recorded.
pub fn load_crash_info(nvs: &EspDefaultNvsPartition) -> Option<CrashInfo> {
    let nvs = EspNvs::new(nvs.clone(), NVS_NAMESPACE, true).ok()?;

    let crash_count = nvs.get_u32(KEY_CRASH_CNT).ok()?.unwrap_or(0);
    if crash_count == 0 {
        return None;
    }

    let last_reset_reason = nvs.get_u8(KEY_LAST_RST_RSN).ok()?.unwrap_or(0);
    let last_crash_ts = nvs.get_u32(KEY_LAST_CRASH_TS).ok()?.unwrap_or(0);

    let mut panic_buf = [0u8; 256];
    let last_panic = nvs
        .get_str(KEY_LAST_PANIC, &mut panic_buf)
        .ok()
        .flatten()
        .map(|s| s.to_string());

    Some(CrashInfo {
        crash_count,
        last_reset_reason,
        last_crash_ts,
        last_panic,
    })
}

/// Clear crash info from NVS (called when user dismisses crash card).
pub fn clear_crash_info(nvs: &EspDefaultNvsPartition) -> Result<()> {
    let nvs = EspNvs::new(nvs.clone(), NVS_NAMESPACE, true)?;
    let _ = nvs.remove(KEY_CRASH_CNT);
    let _ = nvs.remove(KEY_LAST_RST_RSN);
    let _ = nvs.remove(KEY_LAST_CRASH_TS);
    let _ = nvs.remove(KEY_LAST_PANIC);
    info!("Crash info cleared from NVS");
    Ok(())
}

/// Update only the reset reason in NVS (used when boot detection overwrites
/// the panic hook's 0xFF sentinel).
pub fn update_crash_reset_reason(nvs: &EspDefaultNvsPartition, reason: u8) -> Result<()> {
    let nvs = EspNvs::new(nvs.clone(), NVS_NAMESPACE, true)?;
    nvs.set_u8(KEY_LAST_RST_RSN, reason)?;
    Ok(())
}

// ============================================================================
// Location Storage
// ============================================================================

/// Save location to NVS (stored as i32 * 10000 for precision).
pub fn save_location(nvs: &EspDefaultNvsPartition, lat: f32, lon: f32, utc_offset: f32, timezone_name: Option<&str>) -> Result<()> {
    let nvs = EspNvs::new(nvs.clone(), NVS_NAMESPACE, true)?;
    nvs.set_i32(KEY_LATITUDE, (lat * 10000.0) as i32)?;
    nvs.set_i32(KEY_LONGITUDE, (lon * 10000.0) as i32)?;
    nvs.set_i16(KEY_UTC_OFFSET, (utc_offset * 100.0) as i16)?;
    if let Some(tz) = timezone_name {
        nvs.set_str(KEY_TZ_NAME, tz)?;
    }
    info!("Saved location to NVS: lat={:.4}, lon={:.4}, utc_offset={}, tz={:?}", lat, lon, utc_offset, timezone_name);
    Ok(())
}

// ============================================================================
// NvsStorage — implements rhythm_os::Storage for ESP32 NVS
// ============================================================================

/// NVS-backed storage implementing the platform-agnostic `Storage` trait.
pub struct NvsStorage {
    nvs: EspDefaultNvsPartition,
}

impl NvsStorage {
    pub fn new(nvs: EspDefaultNvsPartition) -> Self {
        Self { nvs }
    }
}

impl rhythm_os::storage::Storage for NvsStorage {
    fn load_rooms(&self) -> Result<rhythm_core::room::RoomManager> {
        load_rooms(&self.nvs)
    }

    fn save_rooms(&self, rooms: &rhythm_core::room::RoomManager) -> Result<()> {
        save_rooms(&self.nvs, rooms)
    }

    fn load_config(&self) -> Result<rhythm_os::storage::StoredConfig> {
        let nvs_handle = EspNvs::new(self.nvs.clone(), NVS_NAMESPACE, true)?;
        let defaults = rhythm_core::CurveConfig::default();

        let min_bri = nvs_handle.get_u8(KEY_MIN_BRI).ok().flatten().unwrap_or(defaults.min_brightness);
        let max_bri = nvs_handle.get_u8(KEY_MAX_BRI).ok().flatten().unwrap_or(defaults.max_brightness);
        let min_cct = nvs_handle.get_u16(KEY_MIN_CCT).ok().flatten().unwrap_or(defaults.min_color_temp);
        let max_cct = nvs_handle.get_u16(KEY_MAX_CCT).ok().flatten().unwrap_or(defaults.max_color_temp);
        let solar_noon = nvs_handle.get_i16(KEY_SOLAR_NOON).ok().flatten()
            .map(|v| v as f32 / 100.0)
            .unwrap_or(12.5);

        Ok(rhythm_os::storage::StoredConfig {
            min_brightness: min_bri,
            max_brightness: max_bri,
            min_color_temp: min_cct,
            max_color_temp: max_cct,
            solar_noon_hour: solar_noon,
            width_left_bri: rhythm_core::config::DEFAULT_WIDTH_LEFT_BRI,
            width_right_bri: rhythm_core::config::DEFAULT_WIDTH_RIGHT_BRI,
            width_left_cct: rhythm_core::config::DEFAULT_WIDTH_LEFT_CCT,
            width_right_cct: rhythm_core::config::DEFAULT_WIDTH_RIGHT_CCT,
            shape_p: rhythm_core::config::DEFAULT_SHAPE_P,
            max_dim_steps: rhythm_core::config::DEFAULT_MAX_DIM_STEPS,
        })
    }

    fn save_config(&self, config: &rhythm_os::storage::StoredConfig) -> Result<()> {
        let nvs_handle = EspNvs::new(self.nvs.clone(), NVS_NAMESPACE, true)?;
        nvs_handle.set_u8(KEY_MIN_BRI, config.min_brightness)?;
        nvs_handle.set_u8(KEY_MAX_BRI, config.max_brightness)?;
        nvs_handle.set_u16(KEY_MIN_CCT, config.min_color_temp)?;
        nvs_handle.set_u16(KEY_MAX_CCT, config.max_color_temp)?;
        nvs_handle.set_i16(KEY_SOLAR_NOON, (config.solar_noon_hour * 100.0) as i16)?;
        info!("Config saved to NVS");
        Ok(())
    }

    fn load_location(&self) -> Result<rhythm_os::storage::StoredLocation> {
        let nvs_handle = EspNvs::new(self.nvs.clone(), NVS_NAMESPACE, true)?;

        let lat = nvs_handle.get_i32(KEY_LATITUDE).ok().flatten()
            .map(|v| v as f32 / 10000.0);
        let lon = nvs_handle.get_i32(KEY_LONGITUDE).ok().flatten()
            .map(|v| v as f32 / 10000.0);
        let utc_offset = nvs_handle.get_i16(KEY_UTC_OFFSET).ok().flatten()
            .map(|v| v as f32 / 100.0)
            .unwrap_or(0.0);

        let mut tz_buf = [0u8; 64];
        let timezone_name = nvs_handle.get_str(KEY_TZ_NAME, &mut tz_buf)
            .ok().flatten().map(|s| s.to_string());

        Ok(rhythm_os::storage::StoredLocation {
            latitude: lat,
            longitude: lon,
            utc_offset_hours: utc_offset,
            timezone_name,
        })
    }

    fn save_location(&self, loc: &rhythm_os::storage::StoredLocation) -> Result<()> {
        if let (Some(lat), Some(lon)) = (loc.latitude, loc.longitude) {
            save_location(&self.nvs, lat, lon, loc.utc_offset_hours, loc.timezone_name.as_deref())
        } else {
            Ok(())
        }
    }

    fn load_settings(&self) -> Result<rhythm_os::storage::StoredSettings> {
        let nvs_handle = EspNvs::new(self.nvs.clone(), NVS_NAMESPACE, true)?;

        use rhythm_core::primitives::{DEFAULT_BULB_FADE_MS, DEFAULT_MOTION_TIMEOUT_SECS, DEFAULT_SOFT_OFF_BRIGHTNESS};

        Ok(rhythm_os::storage::StoredSettings {
            bulb_fade_ms: nvs_handle.get_u16(KEY_BULB_FADE_MS).ok().flatten().unwrap_or(DEFAULT_BULB_FADE_MS),
            rhythm_interval_secs: nvs_handle.get_u16(KEY_RHYTHM_INTV).ok().flatten().unwrap_or(60) as u64,
            default_motion_timeout_secs: nvs_handle.get_u16(KEY_DFL_MOT_TOUT).ok().flatten().unwrap_or(DEFAULT_MOTION_TIMEOUT_SECS as u16) as u64,
            power_save: nvs_handle.get_u8(KEY_PWR_SAVE).ok().flatten().unwrap_or(0) != 0,
            soft_off_brightness: nvs_handle.get_u8(KEY_SOB).ok().flatten().unwrap_or(DEFAULT_SOFT_OFF_BRIGHTNESS).clamp(1, 100),
        })
    }

    fn save_settings(&self, settings: &rhythm_os::storage::StoredSettings) -> Result<()> {
        let nvs_handle = EspNvs::new(self.nvs.clone(), NVS_NAMESPACE, true)?;
        nvs_handle.set_u16(KEY_BULB_FADE_MS, settings.bulb_fade_ms)?;
        nvs_handle.set_u16(KEY_RHYTHM_INTV, settings.rhythm_interval_secs as u16)?;
        nvs_handle.set_u16(KEY_DFL_MOT_TOUT, settings.default_motion_timeout_secs as u16)?;
        nvs_handle.set_u8(KEY_PWR_SAVE, if settings.power_save { 1 } else { 0 })?;
        nvs_handle.set_u8(KEY_SOB, settings.soft_off_brightness)?;
        info!("Settings saved to NVS");
        Ok(())
    }

    fn load_hub_credentials(&self) -> Result<HubCredentials> {
        Ok(load_hub_credentials_inner(
            &EspNvs::new(self.nvs.clone(), NVS_NAMESPACE, true)?,
        ))
    }

    fn save_hub_credentials(&self, creds: &HubCredentials) -> Result<()> {
        save_hub_credentials(&self.nvs, creds)
    }

    fn load_hub_registry(&self) -> Result<Option<serde_json::Value>> {
        match load_hue_registry(&self.nvs)? {
            Some(snapshot) => Ok(Some(serde_json::to_value(snapshot)?)),
            None => Ok(None),
        }
    }

    fn save_hub_registry(&self, data: &serde_json::Value) -> Result<()> {
        let snapshot: rhythm_hue::registry::HueRegistrySnapshot = serde_json::from_value(data.clone())?;
        save_hue_registry(&self.nvs, &snapshot)
    }
}

// ============================================================================
// Hue Registry NVS Persistence
// ============================================================================

use rhythm_hue::registry::HueRegistrySnapshot;

/// Save Hue registry snapshot to NVS.
fn save_hue_registry(nvs: &EspDefaultNvsPartition, snapshot: &HueRegistrySnapshot) -> Result<()> {
    let nvs = EspNvs::new(nvs.clone(), NVS_NAMESPACE, true)?;

    let json = serde_json::to_string(snapshot)?;

    if json.len() > 60000 {
        return Err(anyhow::anyhow!(
            "Hue registry JSON too large: {} bytes (max 60000)",
            json.len()
        ));
    }
    if json.len() > 14000 {
        warn!("Hue registry approaching NVS limit: {} bytes", json.len());
    }

    nvs.set_blob(KEY_HUE_REG, json.as_bytes())?;
    info!(
        "Saved Hue registry to NVS: {} rooms, {} bytes",
        snapshot.rooms.len(),
        json.len()
    );

    Ok(())
}

/// Load Hue registry snapshot from NVS.
///
/// Uses a heap-allocated buffer to handle registries up to the 60KB save limit.
fn load_hue_registry(nvs: &EspDefaultNvsPartition) -> Result<Option<HueRegistrySnapshot>> {
    let nvs = EspNvs::new(nvs.clone(), NVS_NAMESPACE, true)?;

    let mut buf = vec![0u8; 16384];

    // Try blob format first (new format)
    if let Ok(Some(blob)) = nvs.get_blob(KEY_HUE_REG, &mut buf) {
        if let Ok(json) = std::str::from_utf8(blob) {
            match serde_json::from_str::<HueRegistrySnapshot>(json) {
                Ok(snapshot) => {
                    info!(
                        "Loaded Hue registry from NVS: {} rooms, {} bytes",
                        snapshot.rooms.len(),
                        blob.len()
                    );
                    return Ok(Some(snapshot));
                }
                Err(e) => {
                    warn!("Failed to parse Hue registry JSON: {}", e);
                }
            }
        }
    }

    // Fallback: try legacy string format
    if let Ok(Some(json)) = nvs.get_str(KEY_HUE_REG, &mut buf) {
        match serde_json::from_str::<HueRegistrySnapshot>(json) {
            Ok(snapshot) => {
                info!(
                    "Loaded Hue registry from NVS (legacy str): {} rooms",
                    snapshot.rooms.len()
                );
                return Ok(Some(snapshot));
            }
            Err(e) => {
                warn!("Failed to parse legacy Hue registry JSON: {}", e);
            }
        }
    }

    Ok(None)
}
