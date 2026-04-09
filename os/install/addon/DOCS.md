# Rhythm OS Add-on Documentation

## Settings

Most lighting curve behaviour is configured via the REST API. The add-on exposes one runtime setting in Home Assistant:

- **Log Level** - Controls how verbose the add-on logs are. Use `debug` when troubleshooting, otherwise keep the default `info`.

Use the **Color Output** setting in the API to choose how commands are sent to your lights:

- `kelvin` (default) – Sends color temperature values (most compatible across bulbs).
- `rgb` – Sends RGB color values.
- `xy` – Sends CIE xy coordinates.

The API also exposes brightness and color-temperature range sliders along with the
morning/evening curve parameters.

## How It Works

This add-on connects to Home Assistant's WebSocket API and listens for events. When a switch is pressed, it automatically turns on lights in the corresponding area with adaptive lighting based on the sun's position.

The adaptive lighting adjusts both brightness and color temperature throughout the day to provide natural, comfortable lighting.
