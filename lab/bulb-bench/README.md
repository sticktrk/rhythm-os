# BulbBench

BulbBench is a standalone laboratory project for reproducible smart-bulb
testing. Its first deliverable rapidly profiles cheaper Matter bulbs against a
measured Hue reference so we can identify where each bulb works, the control
adjustments it needs, and the scenes where it should not be used. The long-term
scope still includes complete optical, electrical, Matter, BLE, reliability,
and grading coverage.

It lives in the CROSS repository so it can reuse Rust dependencies and CI, but
it is not a Rhythm product component and is not linked into the app, appliance,
SDK, or runtime binaries.

The intended operator experience is:

> Insert bulb, identify the sample, close the interlocked chamber, press Go,
> and receive a versioned report with raw evidence and separate performance,
> protocol, capability, and confidence grades.

## Current status

The current phase provides:

- the durable engineering plan and initial test matrix;
- an explicit boundary between laboratory code and shipping Rhythm code;
- versioned bench configuration and safety limits;
- hardware-driver traits for future adapters;
- a focused `matter-profile` suite plan;
- an imported-measurement analyzer that derives a stable brightness floor,
  Hue-relative brightness and CT correction tables, reachable CT limits, Duv
  limitations, and deployment guidance;
- a typed `rhythm-devices` entry and safe database upsert command;
- runtime interpolation of optional brightness and CT corrections so a tested
  Matter bulb can follow Hue-relative logical targets;
- a clearly labeled synthetic fixture and analyzer tests;
- capability-aware plans for the later complete test suites;
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

cargo run -p bulb-bench -- plan --suite matter-profile

cargo run -p bulb-bench -- analyze-profile \
  --input lab/bulb-bench/fixtures/synthetic-hue-vs-budget-matter.json

cargo run -p bulb-bench -- run \
  --config lab/bulb-bench/config/bench.example.json \
  --suite matter-full \
  --sample-id example-bulb-001 \
  --dry-run
```

A command without `--dry-run` is rejected until Phase 1 provides real drivers,
interlock evidence, and a guarded runner.

For a **real, non-synthetic** measurement dataset, update a copy of the device
database for review:

```bash
cargo run -p bulb-bench -- update-rhythm-devices \
  --input path/to/real-measurements.json \
  --database os/rust/core/rhythm-devices/data/devices.json \
  --output /tmp/devices.updated.json
```

After reviewing the diff, the same command may use the canonical database as
its output. Synthetic datasets are always rejected by this command. A built-in
JSON change takes effect when the rpiz software containing it is rebuilt and
deployed.

## Project boundary

BulbBench may use shared dependencies from the root Cargo workspace and the
pure `rhythm-devices` schema crate so exports are compile-time compatible. It
must not depend directly on app UI, appliance processes, or Rhythm's canonical
runtime state. Product code never depends on BulbBench.

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
- [Matter profile MVP](docs/MATTER_PROFILE_MVP.md)
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
