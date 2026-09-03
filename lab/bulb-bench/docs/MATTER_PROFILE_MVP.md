# Matter bulb profile MVP

## The first useful answer

The MVP is not a complete certification lab. It is a fast, evidence-backed
answer to this narrower question:

> Compared with a measured Hue reference, where can this cheaper Matter bulb
> replace it, what commands must Rhythm change, and where should the bulb not
> be used?

For each candidate, the report must identify:

- the minimum command that produces stable, repeatable light;
- the measured brightness curve and a Hue-relative command correction table;
- the measured white-temperature curve and a Hue-relative Kelvin correction
  table;
- white targets that are physically outside the bulb's reachable range;
- residual Duv/tint errors that a Kelvin correction cannot solve;
- important control quirks such as explicit-on requirements, CT commands that
  reset brightness, and a minimum safe command interval;
- suitable and unsuitable deployment roles;
- the exact reviewed `rhythm-devices` entry that makes the findings executable.

The first release produces a profile recommendation, not a letter grade. A
grade would imply broader optical, protocol, reliability, safety, and
multi-sample evidence that the fast run does not yet collect.

## Time target

After a current Hue baseline exists for the exact bench configuration, target
an operator time of less than 20 minutes per candidate:

1. identify, install, close, preflight, and commission the candidate;
2. stabilize at a fixed neutral state;
3. sweep brightness commands and refine the stable floor;
4. sweep representative CT commands at one fixed brightness;
5. run a short Matter behavior sequence;
6. repeat one control point, shut down, and render the profile.

This is an engineering target, not a guaranteed duration. The real nanoLambda
integration time, bulb stabilization, and command settling measurements will
set the final recipe timing. The runner should stop early and report why when
the bulb lacks tunable white, cannot stabilize, overheats, or loses control.

## Hue reference policy

Hue is the operational gold-standard reference, but the software does not
assume it is a perfect physical standard. The reference is itself measured.
Record its exact model, physical sample, firmware, protocol path, warm-up,
geometry, chamber configuration, instrument, and calibration identity.

Capture a Hue baseline:

- at initial bench qualification;
- after any sensor, lining, baffle, socket, geometry, firmware, or recipe
  change;
- at the start of each calibration day while the process is new;
- again when a candidate's closing control point indicates drift.

Candidate and Hue results are comparable only when those identities match.
Keeping the reference baseline separate lets many cheap bulbs be screened
without reinstalling Hue between every sample.

## Minimum measurement recipe

### Brightness curve

At one fixed neutral white state, collect stable output at 100, 75, 50, 25,
10, 5, 3, and 1 percent where accepted. Use additional commands around the
first stable point to resolve the floor. Repeat the floor and one mid-level
point to detect hysteresis or instability.

Normalize each bulb's readings to its own measured maximum before matching
curves. This answers whether a logical Rhythm brightness produces the same
fractional output as Hue without confusing curve shape with absolute lumen
capacity. Preserve absolute readings for later output-capacity comparison.

The correction table maps a logical percentage to the candidate command that
best reproduces Hue's normalized output. Commands below the verified stable
floor are clamped and disclosed.

### Tunable-white curve

At a fixed representative brightness, collect stable spectra at supported
endpoints and applicable 2200, 2700, 3000, 4000, 5000, and 6500 K commands.
For every point retain at least measured CCT and Duv; retain the complete raw
spectrum when the nanoLambda adapter is available.

The correction table maps a logical Kelvin target to the candidate command
that best reproduces the Hue reference's measured CCT. It separately reports:

- the error before correction;
- expected residual error after interpolation or clamping;
- Duv difference from Hue;
- unreachable warm or cool targets.

Duv represents distance from the white locus on a different axis from CCT.
A bulb that is green or magenta relative to Hue may remain unsuitable for
color-sensitive spaces even after its Kelvin mapping is corrected.

### Fast Matter behavior

Use the rpiz **Bulb Audition** rather than rebuilding commissioning and cluster
control in the lab crate. Audition runs the runtime's real plan builder,
records direct readback beside operator observation, and emits the typed
control profile. Capture only the behavior needed to operate the bulb
correctly in this phase:

- accepted capabilities and readback;
- minimum stable brightness behavior;
- explicit-on requirement from an off state;
- whether a CT command reloads or resets the level;
- final-state convergence for brightness plus CT;
- the command interval below which commands are dropped;
- one bounded power-cycle recovery check when safe mains control exists.

Audition also records OnOff, Level, and Color subscription truth and latency;
its adaptive-white scenario rehearses 2200/2700/4000/6500 K through the
resolved route and can persist an operator-validated Kelvin-to-HS curve for
bulbs whose RGB emitters do not render the generic sRGB conversion faithfully.
The optical bench remains responsible for measured Kelvin, Duv, lumen, and
brightness-curve work. Full groups, scenes, OTA, Thread, ecosystem, stress,
and certification testing remains in the long-term Matter phase.

## Immediate deployment output

The analyzer deliberately mirrors the current cloud Matter profile shape.
Rhythm can consume reviewed values for:

- `capabilities.min_brightness`;
- `capabilities.usable_min_kelvin` and `usable_max_kelvin`;
- `quirks.needs_explicit_on`;
- `quirks.recommended_command_spacing_ms`.

Rhythm already orders color before brightness, which accommodates bulbs whose
CT command otherwise resets their level. `rhythm-devices` also supports
optional `control_corrections.brightness` and
`control_corrections.color_temperature` lookup tables. Command adaptation
interpolates the logical Hue-relative target through the matching device curve
before the protocol command is emitted. A missing or invalid curve safely uses
identity mapping, so existing device entries retain their current behavior.

The analyzer includes a complete typed `rhythm_devices_entry` in every report.
Its update command inserts or replaces an exact device match in a selected
`devices.json`, writes atomically, rejects ambiguous matches, and refuses all
synthetic evidence. A reviewer must still inspect real evidence and the JSON
diff. Because the built-in database is compiled into rpiz software, canonical
JSON changes become active in the build that contains them; they are not a
live fleet mutation by themselves.

## Chamber decision for the MVP

Keep the white-lined, light-tight box for comparative testing if the lining is
matte, stable, and reasonably spectrally neutral. Diffuse white reflections can
reduce sensitivity to bulb direction and sensor placement, which helps fast
Hue-relative comparisons. They can also bias the spectrum if the coating is
glossy, colored, dirty, aging, or heated.

Before trusting profiles:

1. baffle the sensor from a direct view if the geometry is intended to collect
   reflected light;
2. mark fixed bulb and sensor positions;
3. measure the Hue reference after rotating and reinstalling it several times;
4. require the resulting brightness, CCT, and Duv variation to stay inside the
   MVP repeatability budget;
5. record the lining and geometry as versioned bench configuration.

This remains a comparative bounce chamber. It is not an integrating sphere and
must not claim absolute total lumens until separately qualified.

## Delivery phases

### 1A — profile analyzer (implemented scaffold)

- import versioned Hue and candidate brightness/CCT observations;
- validate evidence and ignore explicitly unstable points;
- derive floor, correction tables, reachable envelope, Duv limitations, and
  deployment guidance;
- emit and safely upsert a runtime-compatible `rhythm-devices` entry;
- disclose synthetic evidence and hard physical limitations.

### 1B — real measurement collector

- lock the nanoLambda model and implement its adapter;
- add a reusable, versioned Hue baseline artifact;
- call the existing rpiz Matter test/control surface;
- collect the minimum recipe into the analyzer's input schema;
- qualify box repeatability and measurement settling times.

### 1C — guarded one-button run

- add interlocked mains control and independent thermal cutoff evidence;
- implement the deterministic recipe runner and abort path;
- render a concise operator report plus raw JSON/spectral evidence;
- run three repeated profiles of one Hue and one cheap Matter sample.

Exit: an operator installs a bulb, presses Go, and receives the same actionable
profile on repeated runs without manually timing or transcribing measurements.

## Deferred, not abandoned

Absolute output, full spectra/color rendering, flicker, transitions, latency,
power, thermal behavior, lifetime, multi-sample variance, complete Matter,
operational BLE, ecosystem interoperability, and formal grading remain in the
long-term BulbBench plan. They should build on this evidence schema after the
profile MVP is producing real deployment decisions.
