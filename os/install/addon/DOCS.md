# Rhythm OS Add-on Documentation

## Settings

Most lighting behaviour is configured via the REST API, typically through the Rhythm app. The add-on exposes one option in Home Assistant:

- **Log Level** - Controls how verbose the add-on logs are. Use `debug` when troubleshooting, otherwise keep the default `info`.

Curve shape, brightness and color-temperature ranges, light profiles, and mode
transitions are all managed through the REST API (`/api/config`,
`/api/profiles`, `/api/profile-bundle`, `/api/settings`) rather than through
Home Assistant add-on options.

## How It Works

This add-on connects to Home Assistant's WebSocket API and listens for events. When a switch is pressed, it automatically turns on lights in the corresponding area with adaptive lighting based on the sun's position.

The adaptive lighting adjusts both brightness and color temperature throughout the day to provide natural, comfortable lighting.
