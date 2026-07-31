# Local BLE Profile and Driver Conformance

This contract makes support for a new BLE device family reproducible without
giving each vendor its own adapter runtime. It applies to lightweight profiles
(buttons, sensors, and other event-oriented peripherals) and to stateful
drivers such as controllable BLE bulbs, including Hue BLE bulbs.

## Boundary

All Linux central-role device-facing BLE code uses the public `rhythm-ble`
client API. Only `rhythm-ble` may own a BlueZ session or adapter, start device
discovery, or create the Tokio runtime that drives BlueZ. A profile or driver
may own protocol state, but not radio resources. The appliance's peripheral-
role provisioning GATT server is an exact, owned, expiring guard-manifest
exception; it does not discover or control remote devices and is not precedent
for an integration driver.

Use a stable, versioned `profile_id` for the protocol/device family (for
example, `orein.oc02001.button.v1`). A manufacturer/model namespace may avoid
protocol collisions, but it is not a public hub identity or a reason for an
integration crate. Display brand, firmware, and marketing names remain device
metadata. Changing identity proof, wire decoding, counter semantics, or
command encoding requires a new profile version or an explicitly compatible
fixture update.

The registry exposes exactly one current decoder per profile family. A current
descriptor may name older IDs as compatible aliases only when the current
decoder, identity rules, replay tokens, projection, and persisted metadata can
consume those records without reinterpretation. Store load canonicalizes such
aliases to the current ID while preserving the appliance-local public device
ID and Room assignment. An incompatible wire or state change requires an
explicit store migration plus before/after fixtures; registering two live
versions and letting both observe the same advertisement is forbidden because
it can fork identity and emit duplicate events.

Admission is profile-declared. Advertisement-only devices stop after exact
setup/advertisement identity proof; profiles that require GATT declare the
service evidence they accept and connect only for that proof. A future
authenticated challenge must be a new explicit strategy rather than an
implicit GATT side effect. The association deadline covers adapter admission,
scan, connection, service proof, and cleanup as one budget.

The profile parser owns identity normalization. The host compares and routes
the returned identity exactly; it must not case-fold or otherwise reinterpret
profile-owned values. The public canonical ID is random, persisted on first
association, and reused only on that appliance. Never derive it from a raw BLE
identity with an unkeyed hash: vendor/OUI-sized identity spaces are enumerable
and create cross-home correlation.

Simple profile code normally lives with the local-BLE integration. A
controllable bulb or other device with command ordering, observed state,
notifications, security, or reconnection behavior is a driver, not a larger
declarative profile. That driver should normally be a module behind the shared
local-BLE runtime. A separate driver crate needs a documented reason such as
substantial protocol state, dependencies, security isolation, an existing
compatibility surface, or release ownership. A separate crate does not exempt
it from shared runtime ownership or this contract.

The event-profile registry deliberately rejects `light` projections and
advertises only input-device capabilities. Supporting another controllable BLE
bulb therefore requires a rich-driver control/capability seam—commands,
observed state, notifications, reconciliation, and lifecycle—not a profile
that merely calls itself a light. This PR keeps the working Hue BLE beta path
and its `hue_ble@local` public contract as the reference rich driver while
moving its BlueZ ownership underneath `rhythm-ble`; it does not claim that the
generic event host already models arbitrary bulbs.

Event profiles declare every normalized endpoint/action they can emit and may
attach bounded opaque replay evidence keyed by logical stream. The shared
store persists those bytes under the versioned profile ID; it does not assume
an eight-bit counter, a single button, `on_press`, or even that replay evidence
exists. A counterless protocol must explicitly accept its unavoidable
at-least-once behavior in its fixture and product contract.

## Reproducible Fixture Corpus

Each profile/driver keeps sanitized, deterministic trace fixtures with its
tests. Event-profile fixtures are listed in the checked-in corpus manifest, so
the same harness discovers every profile fixture. Fixtures must contain
relative monotonic timestamps, synthetic device IDs and addresses, the minimum
advertisement/GATT bytes needed by the codec, the operation or lifecycle
stimulus, and tagged normalized expected outcomes. Replay evidence is optional
for counterless protocols. Do not commit real MAC addresses, home/customer
IDs, credentials, setup secrets, or unrelated advertisement payloads.
Sanitization must preserve every byte that affects matching, identity,
counters, or decoding.

Every fixture records:

- `schema_version`, `profile_id`, device kind, fixture name, and purpose;
- sanitized device/firmware metadata relevant to compatibility;
- ordered observations and actions using relative `at_ms` timestamps;
- expected identity, normalized events or state, persistence writes, and
  terminal status;
- the capture provenance category (`synthetic`, `sanitized_hardware`, or
  `regression`) without identifying the home or customer.

The generic conformance harness must replay the same fixture more than once
and produce the same result. Wall-clock time, BlueZ object paths, scan order,
and hash-map iteration order must not affect the assertion. Its fixture profile
IDs must cover every registered current profile, so adding a decoder without
adding corpus evidence fails CI.

## Required Cases

Every profile/driver supplies positive happy-path evidence plus the applicable
cases below. Marking a case not applicable requires a short reason beside the
fixture inventory.

### Identity and admission

- Prove identity with the strongest available combination of setup input,
  service/manufacturer data, service UUIDs, and GATT evidence.
- Rotate the resolvable/private BLE address while keeping durable identity and
  assert that one canonical device remains.
- Replay a stale address after rotation and assert that it cannot fork or
  resurrect the device.
- Include negative collision fixtures: nearby devices sharing a name, vendor
  prefix, service UUID, or partial payload must not pair or emit events.
- Reject malformed, truncated, oversized, unsupported-version, and
  semantically impossible frames without panicking or persisting a candidate.

### Ordering and replay

- Replay duplicate observations and profile-owned replay tokens; emit or apply
  each logical event once when the protocol carries sufficient evidence.
- Cover forward gaps without synthesizing missing actions.
- Cover counter wrap with the protocol's exact width and acceptance window.
- Reject late frames outside that window, including frames observed through an
  old address after rotation.
- Restart between observations and prove durable deduplication has the same
  result as an uninterrupted run.

### Persistence and lifecycle

- Recover a previous-stable persisted record and the current record shape.
- Inject write, fsync/rename, truncated-file, and corrupt-record failures. A
  store failure must remain visible and supervised; it must not silently kill
  the only observer or erase the last good state.
- Prefer a caller-visible cancellation contract. When one exists, cancel
  during scan, connection, service discovery, and final persistence;
  cancellation must release its lease/connection and prevent a late successful
  pair from appearing after the caller reports cancellation.
- Until a cancel/status endpoint exists, define and test one bounded server
  SLA instead: the UI is non-dismissible while pairing owns the request, its
  timeout is not shorter than the server's maximum, and the same correlation
  ID reconciles the terminal HTTP/SSE success or failure. A client must not
  abandon the request and later hide a successful pair. Cancellation remains
  the preferred future contract, not an app-only timer.
- Unpair while BlueZ is unavailable. Local canonical/persisted removal must
  succeed, and a tombstone or equivalent policy must prevent automatic
  resurrection until an explicit new pair.
- During factory reset, stop and acknowledge all scanners, connections,
  observers, retries, and writers before deleting BLE state. Assert that no
  task can recreate a file after reset reports success.
- Crash or terminate the shared scanner/worker and prove supervision restarts
  it with bounded backoff while preserving the last durable identity/state.

### Mixed button and bulb workload

At least one integration test runs a button profile and a stateful bulb driver
through the same fake `rhythm-ble` runtime. It must prove:

- exactly one adapter/session/scanner owner exists;
- bounded bulb pairing, reads, writes, notifications, and reconnects do not
  starve button advertisements or create duplicate button events;
- a button observation cannot interrupt an in-flight bulb transaction, and a
  stuck bulb cannot monopolize adapter admission;
- two stable device identities behind the same rich driver use independent
  bounded lanes, while two calls for one identity remain ordered even when
  they come from separately constructed driver handles;
- pairing/enumeration is exclusive with that driver's device lanes; bounded
  admission covers the runtime barrier, driver scope, keyed lane, and adapter
  capacity, while a bounded execution future covers session recovery and GATT
  work; timeout releases the lane and adapter permit for retry;
- quiescence drains active lanes and rejects old-handle work without poisoning
  a fresh post-reset handoff handle;
- failures and retry budgets are isolated per device, and rotating BLE
  addresses are never used as the serialization identity;
- readiness/topology policy is explicit, so an offline accessory does not
  block unrelated Rooms startup indefinitely.

For a controllable bulb, also cover power, brightness, color-temperature or
color (as supported), transition/command ordering, notification subscription,
disconnect/reconnect, and observed-state reconciliation. A successful GATT
write is only a command outcome; physical observation remains authoritative.
Timeouts and ambiguous acknowledgements must stay pending or fail visibly
until observation reconciles them.

## Hardware Receipt

Trace replay is necessary but does not replace one physical-device receipt for
each supported hardware/firmware family. Store or attach a sanitized receipt
containing:

- UTC timestamp, source commit, appliance image/version, BlueZ version, and
  adapter model/firmware;
- `profile_id`, sanitized device model/firmware, and fixture/test case IDs;
- pairing plus cancellation (or bounded terminal reconciliation), unpair, and
  reset results and, when applicable, button event counts or bulb
  command/observed-state results;
- mixed-workload result when support can coexist with another BLE device;
- failures, retries, elapsed bounds, and final cleanup status.

The receipt must omit raw identifiers and secrets. Record an explicit hardware
verification gap in the PR when a receipt cannot be produced; do not present
trace-only coverage as physical compatibility.

## Delivery Gate

A BLE change is review-ready only when:

1. the shared-ownership invariant passes;
2. profile/driver fixtures and the generic fake-runtime suite pass;
3. persistence, cancellation or bounded non-abandonable completion, offline
   unpair, and reset-quiescence behavior are covered when the change
   participates in those lifecycles;
4. the mixed button/bulb workload passes for adapter-affecting changes; and
5. the physical receipt is attached or the remaining bench verification is
   called out as an unchecked release item.

Run the ownership invariant from the repository root:

```bash
.codex/skills/rhythm-feature-delivery/scripts/check-shared-bluez-ownership.sh
```

`tools/check-repo-invariants.sh` runs both this checker and its contract test in
the always-on repository-invariants CI job. Contributor instructions are not
the enforcement boundary.
