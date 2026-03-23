//! Runner DTOs for RhythmRunner in Flutter.

use super::curve::CurveConfigDto;

/// Action type for rhythm runner.
///
/// Maps to ButtonAction in rhythm-core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RhythmActionDto {
    /// Toggle lights on/off with rhythm mode
    OnPress,
    /// Turn off lights
    OffPress,
    /// Reset to current time position
    Reset,
    /// Step up (brighten and cool) along the curve
    StepUp,
    /// Step down (dim and warm) along the curve
    StepDown,
    /// Enable rhythm mode and turn on
    RhythmOn,
    /// Disable rhythm mode (lights unchanged)
    RhythmOff,
    /// Dim up (brightness only, no color temp change)
    DimUp,
    /// Dim down (brightness only, no color temp change)
    DimDown,
}

/// Source/provider for a room.
///
/// Identifies where a room was imported from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomSourceDto {
    /// Unknown source (backwards compatibility)
    Unknown,
    /// Philips Hue bridge
    Hue,
    /// Home Assistant via WebSocket
    HomeAssistant,
    /// ESP32 standalone controller
    Esp32,
}

/// A room managed by the runner.
#[derive(Debug, Clone)]
pub struct RoomDto {
    /// Unique identifier for this room.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Source/provider for this room.
    pub source: RoomSourceDto,
    /// Device IDs assigned to this room.
    pub device_ids: Vec<String>,
    /// Whether rhythm mode is enabled.
    pub rhythm_enabled: bool,
    /// Whether this room is disabled (excluded from kiosk mode).
    pub disabled: bool,
    /// Whether lights are currently on.
    pub lights_on: bool,
    /// Time offset in minutes (from stepping).
    pub time_offset_minutes: f64,
    /// Brightness offset (from dim up/down, -100 to 100).
    pub brightness_offset: f64,
    /// Per-room curve configuration (None uses global config).
    pub curve_config: Option<CurveConfigDto>,
}

impl RoomDto {
    pub fn new(id: String, name: String) -> Self {
        Self {
            id,
            name,
            source: RoomSourceDto::Unknown,
            device_ids: Vec::new(),
            rhythm_enabled: false,
            disabled: false,
            lights_on: false,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
            curve_config: None,
        }
    }

    /// Create a new room with a specified source.
    pub fn with_source(id: String, name: String, source: RoomSourceDto) -> Self {
        Self {
            id,
            name,
            source,
            device_ids: Vec::new(),
            rhythm_enabled: false,
            disabled: false,
            lights_on: false,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
            curve_config: None,
        }
    }
}

/// Complete runner state - holds all rooms.
#[derive(Debug, Clone, Default)]
pub struct RunnerStateDto {
    /// All managed rooms.
    pub rooms: Vec<RoomDto>,
}

/// Command type for light control.
#[derive(Debug, Clone, Copy)]
pub enum LightCommandType {
    TurnOn,
    TurnOff,
}

/// A command to send to a light device.
#[derive(Debug, Clone)]
pub struct LightCommandDto {
    /// The device ID to control.
    pub device_id: String,
    /// The room this device belongs to.
    pub room_id: String,
    /// The command type.
    pub command_type: LightCommandType,
    /// Brightness (0-100), only for TurnOn.
    pub brightness: Option<i32>,
    /// Color temperature in Kelvin, only for TurnOn.
    pub kelvin: Option<i32>,
}

/// Result of handling an action.
#[derive(Debug, Clone)]
pub struct RunnerActionResultDto {
    /// Updated runner state.
    pub state: RunnerStateDto,
    /// Commands to execute.
    pub commands: Vec<LightCommandDto>,
    /// Whether state changed (for persistence).
    pub state_changed: bool,
}
