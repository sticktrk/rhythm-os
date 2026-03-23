# Rhythm OS Add-on Documentation

## Settings

All lighting curve behaviour is configured via the REST API. Use the **Color Output** setting to choose how commands are sent to your lights:

- `kelvin` (default) – Sends color temperature values (most compatible across bulbs).
- `rgb` – Sends RGB color values.
- `xy` – Sends CIE xy coordinates.

The API also exposes brightness and color-temperature range sliders along with the
morning/evening curve parameters.

## How It Works

This add-on connects to Home Assistant's WebSocket API and listens for events. When a switch is pressed, it automatically turns on lights in the corresponding area with adaptive lighting based on the sun's position.

The adaptive lighting adjusts both brightness and color temperature throughout the day to provide natural, comfortable lighting.
