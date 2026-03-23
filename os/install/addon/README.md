# Rhythm OS Add-on for Home Assistant

Adaptive lighting that follows the sun. Rhythm OS automatically adjusts your lights' brightness and color temperature based on the sun's position throughout the day.

## Features

- **Adaptive Lighting**: Adjusts color temperature and brightness based on sun elevation
- **Multi-Protocol Support**: Controls any Home Assistant light entity (ZigBee, Z-Wave, WiFi, Matter)
- **Room-Based Control**: Organizes lights by area with per-room state tracking
- **REST API**: Full API for external control and configuration

## Installation

1. Add the Rhythm OS addon repository to Home Assistant:
   - Navigate to **Settings** → **Add-ons** → **Add-on Store**
   - Click the three dots menu → **Repositories**
   - Add: `https://github.com/sticktrk/rhythm-os-addon`

2. Install the **Rhythm OS** add-on from the store

3. Start the add-on

## How It Works

### Event Flow

1. Switch press triggers a Home Assistant automation
2. The addon receives events via WebSocket subscription
3. The Rust engine calculates optimal lighting values for the current sun position
4. Light commands are sent to all lights in the target area

### WebSocket Connection

The addon connects to Home Assistant's WebSocket API using supervisor authentication. It maintains a persistent connection with automatic reconnection.

### Adaptive Algorithm

The Rust engine uses solar calculations to determine current sun elevation, then maps it through configurable bell curves to produce brightness and color temperature values. Separate morning and evening curves allow fine-tuned control over transitions.

## Troubleshooting

### Lights Not Responding

1. Verify the switch and lights are in the same area
2. Ensure Home Assistant can control the lights directly
3. Review add-on logs for error messages

### Service Calls Not Working

1. Check HA logs for errors
2. Verify the addon is running and connected

## Support

For issues or feature requests: https://github.com/sticktrk/rhythm-os/issues

## License

MIT License - See LICENSE.md for details
