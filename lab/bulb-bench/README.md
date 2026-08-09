# BulbBench

BulbBench is a standalone laboratory project for reproducible optical,
electrical, Matter, BLE, and reliability testing of smart bulbs. It lives in
the CROSS repository so it can reuse Rust dependencies and CI, but it is not a
Rhythm product component and is not linked into the app, appliance, SDK, or
runtime binaries.

The intended operator experience is:

> Insert bulb, identify the sample, close the interlocked chamber, press Go,
> and receive a versioned report with raw evidence and separate performance,
> protocol, capability, and confidence grades.

## Current status

This first phase provides:

- the durable engineering plan and initial test matrix;
- an explicit boundary between laboratory code and shipping Rhythm code;
- versioned bench configuration and safety limits;
- hardware-driver traits for future adapters;
- capability-aware test-suite plans;
- a CLI that validates configuration, displays plans, and produces dry-run
  manifests;
- fail-closed behavior for live hardware execution until the safety state
  machine and real drivers are implemented.

It does **not** energize a socket or control a real bulb yet.

## Quick start

From the CROSS repository root:

```bash
cargo run -p bulb-bench -- validate-config \
  --config lab/bulb-bench/config/bench.example.json

cargo run -p bulb-bench -- plan --suite quick

cargo run -p bulb-bench -- run \
  --config lab/bulb-bench/config/bench.example.json \
  --suite matter-full \
  --sample-id example-bulb-001 \
  --dry-run
```

A command without `--dry-run` is rejected until Phase 1 provides real drivers,
interlock evidence, and a guarded runner.

## Project boundary

BulbBench may use shared third-party dependencies from the root Cargo
workspace. It must not depend directly on app UI, appliance processes, or
Rhythm's canonical runtime state.

Integration occurs through replaceable adapters:

- rpiz control through its authenticated HTTP API;
- nanoLambda through a documented SPI, UART, BLE, or vendor SDK adapter;
- isolated mains control through a locally controlled safety device;
- measurement instruments through their local interfaces;
- optional report publication through an explicit export adapter.

This keeps bench failures, experimental dependencies, and instrument drivers
out of customer software.

## Documentation

- [Engineering plan](docs/PLAN.md)
- [Hardware and chamber design](docs/HARDWARE.md)
- [Test and grading matrix](docs/TEST_MATRIX.md)
- [Example bench configuration](config/bench.example.json)

## Safety

BulbBench will eventually control mains voltage in a thermally enclosed space.
Software is not the primary safety boundary. A real bench requires a rated
socket and contactor, fuse/GFCI protection, protective earth, a door
interlock, a manual emergency stop, independent overtemperature shutdown, and
an enclosure reviewed by a qualified person.

AI may request a bounded high-level test suite. It must never bypass the
deterministic safety state machine or issue unrestricted mains commands.
