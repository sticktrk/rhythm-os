# Hardware and chamber design

## Compartment layout

### Optical compartment

- ceramic, appropriately rated socket on a removable keyed jig;
- bulb under test;
- nanoLambda sensor head;
- baffled high-speed photodiode head;
- chamber and socket temperature probes;
- optional global-shutter camera;
- diffuse white integration surface or matte-black directional surface;
- no status LEDs, exposed colored wiring, labels, or powered controllers.

### Service compartment

- rpiz;
- isolated mains contactor or relay;
- fuse/GFCI and protective-earth termination;
- power analyzer;
- ADC and instrument interface hardware;
- network access point and optional Thread border router;
- manual emergency stop and interlock circuitry;
- independent thermal cutoff;
- runner computer or network connection.

Ventilation between compartments requires a light trap. The temperature-control
air path must not expose the optical cavity to ambient light or create an
unrecorded change in bulb cooling.

## Required safety chain

Socket power requires every independent condition below:

1. rated fuse/GFCI and protective earth are intact;
2. manual emergency stop is released;
3. door interlock is closed;
4. independent thermal cutoff is below its threshold;
5. hardware watchdog is alive;
6. a short-duration power lease is active;
7. the deterministic runner remains within recipe time and temperature limits.

The contactor must drop out on loss of controller power. The emergency stop,
door interlock, and thermal cutoff must not rely solely on the runner process.

## Optical modes

### Comparative diffuse chamber

This is the starting use for an existing white-lined box. Add a sensor baffle,
fixed jigs, reference lamp, dark shutter, and repeatability qualification. It
can support internally consistent rankings but must not imply traceable total
luminous flux.

Qualification runs should cover:

- ten remove/reinstall cycles of the reference lamp;
- four lamp orientations;
- repeated dark baselines;
- cold and warm chamber conditions;
- low, medium, and high light levels;
- spectral shape, intensity, CCT, and Duv drift;
- sensor saturation and exposure-bracketing behavior.

### Calibrated integrating mode

Absolute total spectral and luminous flux requires:

- a properly sized 4-pi integrating geometry;
- diffuse, spectrally characterized, low-fluorescence coating;
- center mounting and sensor baffle;
- a traceable reference lamp or calibrated flux introduction;
- auxiliary-lamp self-absorption correction;
- spatial-response and system-level spectral calibration;
- a documented uncertainty budget.

### Directional mode

A removable matte-black tunnel with fixed bulb-to-sensor distance can measure
directional chromaticity, intensity proxies, beam behavior, and angular
variation. Directional results must remain distinct from total-flux results.

## Instrument additions

The spectrometer alone cannot provide the complete matrix. Plan for:

- fast photodiode and at least 100 kS/s acquisition for temporal-light and
  command-to-photon timing;
- true-RMS power analyzer with useful low-power standby resolution;
- chamber and socket RTD or thermocouple measurement;
- independent mains switching capable of repeated cold and hot cycles;
- a BLE packet sniffer for advertisement/GATT evidence;
- controlled RF attenuation for range and coexistence work;
- Thread RCP and border router for Matter-over-Thread;
- reference lamp and external cross-validation.

## RF considerations

An optical enclosure can produce an uncontrolled RF environment. A metal
integrating sphere may attenuate Matter, Wi-Fi, Thread, or BLE. Functional
optical runs can use a repeatably positioned internal antenna or a known
external path, but formal RF performance belongs in a separately controlled
RF fixture. Record the antenna, access point, channel, distance, attenuation,
and enclosure state for every protocol run.
