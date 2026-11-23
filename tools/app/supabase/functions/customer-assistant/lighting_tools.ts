const integer = (minimum: number, maximum: number) => ({
  type: "integer",
  minimum,
  maximum,
});
export const lightingTools = {
  adjust_room_brightness: {
    description:
      "Change adaptive brightness by percentage points. A little brighter is +5. Can turn the room on.",
    fields: { delta_percent: integer(-100, 100) },
  },
  adjust_room_warmth: {
    description:
      "Set a Kelvin target while preserving brightness. Lower values are warmer.",
    fields: { kelvin: integer(500, 25000) },
  },
  control_room_power: {
    description:
      "Turn on adaptive lighting, use the configured off behavior, or turn every managed lamp fully off.",
    fields: { power: { type: "string", enum: ["on", "off", "hard_off"] } },
  },
  set_room_color: {
    description:
      "Try a sRGB color for one minute at current brightness. May turn the room on.",
    fields: {
      red: integer(0, 255),
      green: integer(0, 255),
      blue: integer(0, 255),
    },
  },
  try_room_offset: {
    description:
      "Try an earlier (negative minutes) or later look on this room's curve for one minute.",
    fields: { offset_minutes: integer(-720, 720) },
  },
} as const;
