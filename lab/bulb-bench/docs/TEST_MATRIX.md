# Test and grading matrix

## Profile MVP gate

The first automated gate is intentionally smaller than the complete matrix
below. It compares a candidate Matter bulb with a current, same-bench measured
Hue baseline and answers where the candidate is deployable.

- normalized brightness curve at 100, 75, 50, 25, 10, 5, 3, and 1 percent as
  supported, plus refinement and repetition at the stable floor;
- CCT and Duv at supported endpoints and applicable 2200, 2700, 3000, 4000,
  5000, and 6500 K commands at one fixed representative brightness;
- explicit-on, CT-level reset, command spacing, readback, convergence, and one
  safe recovery check;
- Hue-relative brightness and CT correction tables;
- reachable envelope, uncorrectable tint, suitable/avoid roles, evidence
  validity, and a typed `rhythm-devices` export.

This gate produces a profile recommendation, not the formal grades defined
later in this document. See [Matter bulb profile MVP](MATTER_PROFILE_MVP.md).

## Sample identity and claims

- brand, model, retail SKU, GTIN/UPC, region, voltage, frequency, wattage;
- bulb shape, diffuser, socket/base, dimensions, mass, manufacture lot;
- firmware, hardware revision, Matter vendor/product IDs, certification ID;
- BLE company ID, advertised services, GATT fingerprint;
- claimed lumens, CCT range, CRI, colors, lifetime, protocols, and app needs;
- packaging and bulb photographs with setup codes redacted.

## Optical measurement grid

### Brightness

Test 100, 75, 50, 25, 10, 5, 3, and 1 percent plus the discovered minimum
stable level. Calculate measured output versus requested output, monotonicity,
dead zones, hysteresis, normalized error, usable dynamic range, repeatability,
and the level at which color or flicker quality materially changes.

### Tunable white

Test the supported endpoints and applicable 2200, 2700, 3000, 3500, 4000,
5000, and 6500 K targets across representative brightness levels. Calculate:

- requested versus measured CCT and percent error;
- CIE 1931 xy and CIE 1976 u-prime/v-prime;
- Duv and distance from the target locus;
- SPD, photopic output, spectral shift, and channel crossover artifacts;
- CRI Ra, R1-R15 including R9;
- TM-30 Rf, Rg, local fidelity/chroma/hue shift, and color-vector graphic;
- S-, M-, and L-cone-opic, rhodopic, and melanopic quantities under CIE S 026;
- warm-up and temperature-induced color drift.

### Color

Test RGB and CMY primaries/secondaries, neutral white, a larger gamut grid, and
representative targets at several brightness levels. Calculate:

- requested versus measured xy/u-prime/v-prime error;
- achievable gamut area and gamut contraction at low brightness;
- hue and saturation mapping error;
- primary-channel crosstalk and clipping;
- RGB-generated white versus dedicated-white-channel quality;
- repeatability after color-mode changes.

Capture at least three spectra per point. Randomize or block-randomize the
matrix, and return periodically to a fixed 4000 K/50 percent control point to
measure drift.

## Temporal light and control dynamics

- powered-off dark waveform;
- steady-state waveform at every brightness level and major light mode;
- dominant frequencies, FFT, modulation depth, percent flicker, flicker index;
- PstLM, SVM, and other metrics supported by the validated implementation;
- command acknowledgement latency;
- command-to-first-photon latency;
- command-to-90-percent and settle latency;
- requested versus physical transition duration;
- overshoot, undershoot, stepping, stalls, and non-monotonic transitions;
- rapid command thresholds and final-state convergence;
- state readback and subscription lag relative to physical output.

## Electrical and thermal

- voltage, current, watts, apparent power, power factor, and energy;
- standby draw when logically off;
- inrush and startup behavior;
- current and voltage waveform/THD when instrumented;
- efficacy only when absolute luminous flux is valid;
- chamber, ambient, socket, and bulb-base temperatures;
- warm-up time, steady state, output droop, spectral drift, and cool-down;
- thermal shutdown or derating behavior;
- audible driver noise by mode and brightness when instrumented.

Any odor, arcing, smoke, enclosure damage, unsafe surface temperature, or
independent cutoff activation terminates the run and produces **Do Not
Recommend**, not a numerical grade.

## Power and reliability

- cold start, warm start, and hot restart;
- logical off/on state restoration;
- external mains power restoration;
- repeated power cycles with controlled off time;
- bounded brownout/voltage work only with appropriate equipment and review;
- command soak and color/brightness cycling;
- disconnect, network loss, AP reboot, DHCP renewal, IPv6/mDNS recovery;
- internet outage while local control remains available;
- process/controller restart and state reconciliation;
- firmware update and recovery where supported;
- repeated run and multiple-retail-sample variance.

## Matter

- QR and manual commissioning; BLE and NFC commissioning when claimed;
- factory reset, failed commissioning cleanup, recommissioning time and rate;
- attestation and DCL evidence without publishing secrets;
- descriptor, endpoint, device type, cluster, feature map, accepted command,
  and attribute-list coherence;
- OnOff, Level Control, Color Control, transitions, and physical readback;
- subscriptions, data versions, events, groups, scenes, and binding as claimed;
- multiple fabrics, multi-admin, fabric removal, and Joint Fabric as supported;
- session recovery, duplicate/lost command handling, and rapid command limits;
- OTA discovery, update, interruption, rollback, and final firmware identity;
- Matter-over-Wi-Fi and Matter-over-Thread reported separately;
- ecosystem receipts from Rhythm, Apple, Google, Alexa, SmartThings, Home
  Assistant, and other selected controllers;
- official Matter Test Harness results reported separately from the BulbBench
  compatibility grade.

## BLE

First classify BLE as Matter commissioning, proprietary operational GATT, or
Bluetooth Mesh. Then cover what is applicable:

- advertisements, interval, payload, discovery latency, RSSI, and identity;
- address rotation and resistance to mistaken identity;
- pairing, encryption, authorization, bonding, and unauthorized access;
- service, characteristic, descriptor, property, and MTU inventory;
- reads, writes, write-without-response, notifications, and ordering;
- protocol errors, malformed input, timeouts, and retry budgets;
- command acknowledgement versus physical response;
- disconnect/reconnect and power-cycle recovery;
- multiple bulbs, per-device serialization, and bounded concurrency;
- coexistence with Wi-Fi, Thread, and other BLE devices;
- controlled attenuation/range and packet-sniffer evidence;
- Android/iOS behavior when mobile apps are part of normal operation;
- Bluetooth qualification status separately from BulbBench results.

## Grade model

Publish four independent outputs.

### Capability class

- L0: on/off;
- L1: dimmable white;
- L2: tunable white;
- L3: full color;
- separate protocol and transport badges.

### Rhythm performance score

- optical and color quality: 30 percent;
- dimming and transitions: 20 percent;
- reliability and recovery: 15 percent;
- command/state accuracy and physical latency: 15 percent;
- electrical, temporal-light, and thermal quality: 10 percent;
- interoperability: 10 percent.

Initial letter bands are A+ 95-100, A 90-94, B 80-89, C 70-79, D 60-69,
and F below 60. Revisit thresholds after a representative baseline corpus.

A legitimately absent capability is N/A and remains visible in capability
class. A claimed capability that fails receives zero. Safety failures are not
averaged away.

### Protocol grades

Matter and BLE receive independent grades so optical quality cannot conceal a
bad protocol implementation and protocol quality cannot conceal bad light.

### Confidence

- Exploratory: one sample and one run;
- Verified: repeated runs of one sample on multiple days;
- Gold: at least three retail samples, repeated runs, daily reference checks,
  and representative external cross-validation.

## Report artifacts

- executive grade and capability badges;
- validity, exclusions, uncertainty, and confidence;
- white accuracy, Duv, chromaticity, brightness, gamut, SPD, CRI, TM-30, and
  alpha-opic plots;
- temporal waveform, FFT, transition, latency, power, and thermal plots;
- Matter/BLE conformance and interoperability tables;
- recovery and endurance results;
- anomalies, known quirks, and recommended control strategy;
- exact versions, calibration evidence, raw artifact hashes, and reproducible
  calculation inputs.
