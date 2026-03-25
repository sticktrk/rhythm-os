<p align="center">
  <img src="install/addon/logo.png" alt="Rhythm OS" width="200">
</p>

<h1 align="center">Rhythm OS</h1>

<p align="center">
  <strong>A modular lighting operating system.</strong><br>
  <a href="https://rhythm.lighting">rhythm.lighting</a>
</p>

<p align="center">
  <a href="LICENSE.md"><img src="https://img.shields.io/badge/license-Apache--2.0-blue.svg" alt="License"></a>
  <a href="https://github.com/sticktrk/rhythm-os/actions/workflows/ci.yml"><img src="https://github.com/sticktrk/rhythm-os/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
</p>

Rhythm OS is a headless, curve-driven lighting engine written in Rust. It continuously evaluates a lighting curve to produce the right **brightness** and **color temperature** for every moment of the day — then pushes those values to your lights. The default curve is a Gaussian shaped by solar position, but the curve engine is pluggable: implement the `LightCurveModule` trait and drop in any shape you want.

The same core runs everywhere: on an **ESP32** microcontroller, a **macOS/Linux** server, or a **Home Assistant** add-on. Hub-agnostic — works with any lighting product that has an integration crate. The entire system is controlled through a REST API. Bring your own frontend, or use the [Rhythm app](https://apps.apple.com/us/app/rhythm-lighting/id6758312802).

## How it works

Rhythm OS manages your lights through **curves** — continuous functions that define brightness and color temperature over the course of a day. At any moment, the engine knows the exact values your lights should be at. When you press a button, lights come on at those values. Step dimming moves along the curve instead of arbitrary percentages.

- **Curve-driven** — lights follow a smooth function, not manual presets or timers
- **Pluggable curve engine** — the default is a Gaussian shaped by solar position, but implement `LightCurveModule` for any shape
- **Room-level control** — rooms, devices, buttons, motion sensors, group addressing
- **Hub-agnostic** — add lighting products by implementing a small set of traits
- **REST API + SSE** — full programmatic control with real-time state streaming

## Platforms

| Target | Crate | Notes |
|--------|-------|-------|
| macOS / Linux | `rhythm-server` | CLI server with HTTP API and mDNS discovery. |
| Home Assistant | `rhythm-addon` | Add-on with ingress support. Auto-configures from HA Supervisor. |
| ESP32-C6 | `rhythm-esp32` | Standalone controller. WiFi + BLE provisioning, OTA updates. |

## Quick start

Download a pre-built binary from [Releases](https://github.com/sticktrk/rhythm-os/releases), or build from source:

```bash
cargo build -p rhythm-server --release
cargo test
```

See [install/](install/) for platform-specific setup (macOS, Linux, Home Assistant, ESP32).

## REST API

Rhythm OS exposes a complete REST API for managing lights, rooms, and curves. All platforms share the same endpoints.

| Category | Endpoints | Description |
|----------|-----------|-------------|
| **State** | `GET /api/state`, `GET /api/rooms/state` | Full snapshot or lightweight room state |
| **Events** | `GET /api/events` | SSE stream (room state, motion, hub status, config changes) |
| **Rooms** | `PUT /api/rooms`, `PUT /api/rooms/action`, `PUT /api/rooms/brightness` | Create rooms, dispatch actions, set brightness |
| **Devices** | `PUT /api/devices`, `PUT /api/sensors` | Register lights, buttons, and motion sensors |
| **Config** | `GET /api/config`, `PUT /api/config`, `PUT /api/location` | Read/write curve config and location |
| **Settings** | `GET /api/settings`, `PUT /api/settings` | Fade duration, update interval, power save |
| **Hub** | `PUT /api/hub/credentials`, `POST /api/sync` | Connect to a lighting hub, trigger re-discovery |

See [CLAUDE.md](CLAUDE.md#api-endpoints) for the full endpoint reference.

## Architecture

Rhythm is built as a layered crate architecture. Each layer has a single responsibility, and integrations are added by implementing a small set of traits — no forks, no monkeypatching.

```
┌─────────────────────────────────────────────────────┐
│  Binaries (platform I/O)                            │
│  rhythm-esp32 · rhythm-server · rhythm-addon        │
├─────────────────────────────────────────────────────┤
│  OS layer (hub-agnostic business logic)             │
│  rhythm-os                                          │
├─────────────────────────────────────────────────────┤
│  Integrations (product-specific crates)             │
│  rhythm-hue  ·  rhythm-ha  ·  ...                   │
├─────────────────────────────────────────────────────┤
│  Core (pure algorithms, no I/O)                     │
│  rhythm-core                                        │
└─────────────────────────────────────────────────────┘
```

### Crates

| Crate | Purpose |
|-------|---------|
| **rhythm-curve** | Pluggable lighting curve contract. Defines the `LightCurveModule` trait for custom curve shapes. |
| **rhythm-core** | Solar calculations, curve engine, color science, runtime orchestration. Pure algorithms with zero I/O — runs on any platform. Feature-gated for `tokio` (async) or `blocking` (embedded). |
| **rhythm-os** | Hub-agnostic business logic: room management, event loop, command handling, persistence. Knows nothing about Hue — dispatches through traits. |
| **rhythm-hue** | Philips Hue V2 integration. Implements `LightController`, `HubRegistry`, and `HubProvider`. Platform-abstracted via `HueTransport` trait — same logic on ESP32 and desktop. |
| **rhythm-ha** | Home Assistant integration. Implements `LightController`, `HubRegistry`, and `HubProvider` for HA's WebSocket API and ZHA events. |
| **rhythm-server** | macOS/Linux CLI server. HTTP API + mDNS discovery. Runs the full engine as a native process. |
| **rhythm-addon** | Home Assistant add-on binary. Connects to HA via WebSocket, serves HTTP API with ingress support. |
| **rhythm-esp32** | Standalone ESP32-C6 firmware. WiFi, BLE provisioning, NVS persistence, OTA updates. |

### Dependency graph

```
rhythm-curve ← base, no rhythm-* deps
    ↑
rhythm-core  ──→ rhythm-curve
    ↑
rhythm-os    ──→ rhythm-core + rhythm-curve
rhythm-hue   ──→ rhythm-core + rhythm-os
rhythm-ha    ──→ rhythm-core + rhythm-os
    ↑
rhythm-server ──→ rhythm-os + rhythm-hue + rhythm-ha
rhythm-addon  ──→ rhythm-os + rhythm-hue + rhythm-ha
rhythm-esp32  ──→ rhythm-os + rhythm-hue
```

## Extending: Add a new integration

Every lighting product gets its own crate (`rhythm-{name}`) implementing four things:

1. **`LightController`** — send lighting commands to devices
2. **`HubRegistry`** — track device-to-room mappings
3. **`HubProvider`** — configure credentials and connection
4. **Event translation** — convert native device events into generic `HubEvent`s

Then wire it into a binary's dispatch table:

```rust
match hub_type.as_str() {
    "hue"  => &HUE_PROVIDER,
    "lifx" => &LIFX_PROVIDER,  // new integration
    _ => /* default */
}
```

No changes to rhythm-core or rhythm-os required. See [INTEGRATIONS.md](INTEGRATIONS.md) for the full contract and a crate template.

**Current integrations:** Philips Hue (V2 API, SSE events, button/motion sensors), Home Assistant (WebSocket API, ZHA events)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for build instructions, code style, and how to submit changes.

## Community

- [rhythm.lighting](https://rhythm.lighting) — Project homepage
- [Discord](https://discord.gg/TUvSrtRt) — Help, setups, feature discussion
- [GitHub Issues](https://github.com/sticktrk/rhythm-os/issues) — Bugs and feature requests

## License

Licensed under the Apache License, Version 2.0. Copyright 2025 DT Concepts, LLC. See [LICENSE](LICENSE.md) for details.
