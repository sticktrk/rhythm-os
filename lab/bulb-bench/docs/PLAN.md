# BulbBench engineering plan

## Outcome

BulbBench will become a one-button, evidence-first smart-bulb laboratory. The
first value milestone is deliberately narrower: profile inexpensive Matter
bulbs against a measured Hue reference and quickly produce the brightness/CT
adjustments, residual flaws, and deployment boundaries Rhythm needs. The
long-term system will add complete physical, protocol, electrical, BLE,
reliability, and grading evidence.

The authoritative result is the physical observation. A successful Matter or
BLE command acknowledgement is useful protocol evidence, but does not prove
that the bulb produced the requested light.

## Principles

1. **Separate laboratory code from customer software.** Instrument drivers,
   experimental SDKs, recipes, and large evidence artifacts remain under
   `lab/bulb-bench`.
2. **Keep hardware execution deterministic.** AI may select and analyze a
   bounded suite. A state machine owns timing, retries, safety, and mains.
3. **Fail closed.** Missing interlocks, sensors, calibration evidence, storage,
   or hardware acknowledgements prevent socket power.
4. **Retain raw evidence.** Every grade can be reproduced from versioned input
   data and a versioned scoring model.
5. **Separate capability, quality, protocol, and confidence.** A simple white
   bulb should not be penalized for lacking RGB; a claimed RGB feature that
   fails must affect its result.
6. **Do not overclaim.** Comparative chamber results are not absolute lumens;
   compatibility tests are not Matter or Bluetooth certification.

## System shape

The optical cavity contains only the bulb, socket, optical sensor heads,
temperature probes, baffles, and an optional camera. The rpiz, relays, power
analyzer, ADC, networking, and runner live in a separate service bay.

The runner coordinates these adapter families:

- `ProtocolController`: rpiz Matter first, then operational BLE drivers;
- `MainsController`: isolated, locally controlled, leased socket power;
- `Spectrometer`: the selected nanoLambda model;
- `FastLightMeter`: high-speed photodiode and ADC for temporal light;
- `PowerAnalyzer`: electrical measurements and inrush evidence;
- `EnvironmentMonitor`: chamber and socket temperatures;
- `EvidenceSink`: immutable local artifacts and later curated publication.

## One-button lifecycle

1. Scan or enter bulb model and sample identity.
2. Install the bulb and close the chamber.
3. Verify interlocks, emergency stop, independent thermal cutoff, instrument
   connectivity, calibration status, temperature, storage, and recipe limits.
4. Capture powered-off dark baselines.
5. Energize the socket with a bounded power lease.
6. Commission or reconnect the selected control protocol.
7. Capture identity, firmware, capabilities, endpoints, services, clusters,
   attributes, and accepted controls.
8. Stabilize a neutral reference state.
9. Execute the capability-aware optical and protocol matrix.
10. Repeat fixed reference points to quantify drift.
11. Exercise bounded network and power recovery.
12. De-energize the socket and verify shutdown.
13. Persist evidence, compute metrics, assign grades, and render reports.

## Optical strategy and the white lining

The exterior must be light-tight, but darkness does not determine the correct
interior reflectance.

- A matte-black fixed geometry is appropriate for directional measurements.
- A purpose-built, highly diffuse, spectrally neutral white integrating
  surface is appropriate for spatially integrated measurements.
- Ordinary white paint creates a useful comparative bounce chamber only after
  positional repeatability, spectral bias, thermal behavior, and drift are
  characterized.

The initial white-lined box should therefore be labeled **comparative mode**.
Absolute total luminous flux requires a calibrated integrating sphere or a
validated equivalent with a reference lamp, a baffle, spatial-uniformity
evidence, and self-absorption correction.

## Phases

### Phase 0 — scaffold and decisions

- Land this standalone project and plan.
- Confirm the nanoLambda model, wavelength range, power calibration, transport,
  and automation API.
- Fix the supported mains voltage, maximum bulb power, socket types, chamber
  volume, and maximum temperature.
- Decide whether early results are comparative only or intended for absolute
  photometry.
- Select the relay/contactor, power analyzer, temperature monitor, fast
  photodiode/ADC, and reference lamp.

Exit: reviewed hardware diagram, parts list, safety review, and accepted data
schemas.

### Phase 1 — safe quick suite

- Treat Hue as a measured operational reference, with a versioned baseline for
  the exact bench geometry and calibration period.
- Measure the candidate's stable brightness floor and curve, representative CT
  curve, Duv difference, and only the Matter behaviors needed for correct
  control.
- Produce suitable/avoid deployment guidance and a reviewed `rhythm-devices`
  entry containing directly usable Matter hints and brightness/CT correction
  lookup tables.
- Implement one nanoLambda driver, reuse the existing rpiz Matter tester, and
  add immutable measurement evidence.
- Add the interlocked mains/environment drivers and guarded one-button runner
  after manual/imported measurement collection proves the analysis loop.

Exit: one real candidate completes a repeatable Hue-relative profile in a
target of under 20 minutes and the report identifies both usable roles and hard
limits. See [Matter bulb profile MVP](MATTER_PROFILE_MVP.md).

### Phase 2 — optical and electrical characterization

- Qualify the chamber with a reference source and repeated positioning tests.
- Add the full brightness/CCT/color grid and repeated measurements.
- Add a fast photodiode for physical onset, transition, PWM, and flicker.
- Add power, standby, inrush, power factor, and thermal measurements.
- Calculate CIE color, CRI, TM-30, CIE S 026, temporal-light, dimming, and drift
  metrics with explicit uncertainty and validity flags.

Exit: a full optical report can distinguish color accuracy, color quality,
dimming quality, temporal-light behavior, and electrical efficiency.

### Phase 3 — Matter laboratory

- Correlate rpiz command, protocol acknowledgement, readback, subscription,
  photodiode onset, spectrum settle, power, and temperature under one run ID.
- Automate commissioning, reset, fabric, group, scene, subscription, rapid
  command, power restoration, AP restart, packet loss, and internet-outage
  cases.
- Add a Thread RCP and controlled border router before claiming Matter-over-
  Thread coverage.
- Integrate the official Matter Test Harness separately from the product
  compatibility grade.
- Add ecosystem interoperability receipts.

Exit: each bulb receives a separate Matter grade backed by protocol and
physical evidence.

### Phase 4 — BLE laboratory

- Classify BLE as Matter commissioning, proprietary operational GATT, or
  Bluetooth Mesh.
- Add advertisement and GATT inventory capture.
- Add versioned proprietary bulb-driver plugins where operational control is
  understood.
- Measure discovery, connection, write, notification, physical latency,
  disconnect, reconnect, power-cycle, coexistence, security, and concurrency.
- Add a packet sniffer and controlled attenuation for repeatable RF behavior.

Exit: supported operational BLE families receive a physical-behavior grade;
unknown devices still receive an honest inventory and commissioning report.

### Phase 5 — publication matrix

- Establish stable bulb-model and physical-sample identities.
- Require repeated runs and multiple retail samples for published confidence.
- Retest after firmware changes.
- Store large waveforms and spectra in object storage and indexed metadata in
  a relational database.
- Publish human-readable comparisons and a machine-readable capability API.
- Export reviewed quirks and recommended control strategy to Rhythm through a
  deliberate adapter rather than a direct laboratory dependency.

Exit: a growing, reproducible bulb capability and performance matrix.

## Data contract

Every run records:

- run, recipe, scoring, source, and schema versions;
- bench, chamber, reference lamp, instrument, and calibration identities;
- bulb model, physical sample, firmware, protocol, and claimed capabilities;
- safety preflight and shutdown receipts;
- monotonic and wall-clock timestamps for commands and observations;
- raw spectra, temporal waveforms, electrical and thermal time series;
- protocol request, acknowledgement, readback, notification, and error data;
- environment and reference-point drift;
- calculations, uncertainty, exclusions, anomalies, grades, and confidence;
- hashes and locations for large immutable artifacts.

Public exports remove setup codes, network credentials, raw serial numbers,
MAC addresses, private keys, and identifying packet-capture material.

## AI boundary

AI-facing operations should be high-level and bounded:

```text
preflight()
commission(protocol)
run_suite(name, sample_id)
set_light(target, bounded_timeout)
measure_spectrum()
capture_transition()
power_cycle(count, off_ms, maximum_temperature)
abort()
generate_report(run_id)
```

The runner validates each request against the active recipe and safety lease.
No AI call can disable the door interlock, emergency stop, thermal cutoff,
maximum on-time, or evidence logging.

## Initial implementation decisions still required

- Exact nanoLambda model and whether it is power calibrated
- 120 V only versus additional regions and socket families
- Maximum bulb dimensions and wattage
- Comparative chamber versus purchased/built calibrated integrating sphere
- Matter-over-Wi-Fi-only initial scope versus immediate Thread hardware
- Fast photodiode, ADC sample rate, power analyzer, relay, and reference source
- Calibration partner for external cross-validation
