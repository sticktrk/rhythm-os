# State Authority and Compatibility

Selective read scopes, omission/empty cache semantics and mixed-version behavior
are specified in [Selective state reads](selective-state.md).

This document captures recurring distributed-state rules for the app, SDK, server, appliance, and integrations.

## Authority ladder

From strongest to weakest:

1. Physical or integration observation
2. Canonical server state derived from that observation
3. Server command outcome
4. Server command acceptance or queueing
5. App optimistic state

Queue acceptance is not physical success. `pending_dispatch` should span the user action through an integration outcome. A failure must clear pending state and remain visible long enough to explain what happened.

## Ordering

SSE, polls, and action-response snapshots may arrive out of order. New or changed protocols should carry:

- a monotonic state revision or comparable generation;
- the originating command or correlation ID when applicable;
- observation time, distinct from serialization time.

Clients must not let an older snapshot raise pending state or replace a newer physical observation. Compatibility fallbacks may remain for older appliances, but new contracts should not depend on timing heuristics.

## Identity

Use durable proof at every boundary:

- Home/server: server instance ID plus owner proof
- Canonical device: canonical node ID
- Hue: resolve the native light or grouped-light resource at the adapter boundary
- Matter: node, endpoint, and fabric identity
- Home Assistant: entity identity scoped to the configured hub

Never merge identity by IP address, display name, cached reachability, or a native device container that is not itself controllable.

## Failure isolation

Scope timeouts, cooldowns, retry budgets, queues, and liveness to the smallest responsible target. One hub or device must not block unrelated devices, HTTP requests, periodic work, debug-bundle creation, or app reconnect.

After restart, reconcile persisted state with the external integration. When older persisted data can preserve a defect, add a startup repair or migration and test both old and repaired forms.

## Hue room topology projection

Hue automation suppression, room-membership projection, and grouped-light dispatch are separate authorities:

- automation suppression requires its existing complete room review;
- topology projection is an additional capability-gated opt-in and defaults off;
- Rhythm's canonical room assignment commits even when Hue is unavailable;
- the desired projection is durable canonical state, so restart or reconnect can retry it;
- Rhythm may change only Hue light membership and rooms carrying an explicit stable-bridge ownership receipt;
- user-created Hue rooms, zones, scenes, automations, and accessory membership are preserved;
- ambiguous native recovery is an attention state, never a name-based ownership guess;
- grouped dispatch stays fenced until exact Hue membership and grouped-light identity are read back;
- an offline, partial, or conflicting projection continues through individual-light dispatch and must not fence unrelated integrations.

## App/appliance skew

The app and appliance release independently. Contract-changing work should cover this matrix where relevant:

| Client | Appliance | Expected behavior |
|---|---|---|
| Current | Current | Full capability path |
| Current | Previous stable | Compatibility fallback or clear unsupported state |
| Previous supported | Current | Existing fields and actions continue to work |
| Remote client | Current | Same auth and state semantics as LAN, modulo transport capability |

Prefer additive fields with tolerant readers. Preserve unknown enum values when feasible. Gate new behavior on advertised capabilities or schema versions rather than version-string guesses.

## Journey correlation

Reuse one external journey ID across app actions, HTTP `X-Request-Id`, BLE requests, OTA attempts, integration calls, activity records, and debug-bundle summaries when those layers participate in one user operation. Subsystems may keep native IDs, but record their relationship to the journey ID.

At minimum, field verification tools must send a stable `X-Request-Id` across their requests and include it in the receipt.

## Operational signals

Prefer measurements that correspond to the real user journey:

- completion or failure count by onboarding stage;
- command enqueue-to-physical-confirmation latency by integration;
- SSE disconnect and recovery duration;
- OTA attempt, installed version, probation, and rollback result;
- remote tunnel readiness and recovery;
- unassigned-device age and resolution.

Debug bundles remain the authoritative incident artifact. Add a signal when the bundle cannot reconstruct the failure without guessing.
