# Selective state reads

`GET /api/state` accepts an optional comma-separated `include` query. An
unqualified request retains the full legacy snapshot. New clients can request
`include=base` (also the meaning of an explicitly empty include) without loading
the node catalog. Authentication and owner/support permissions are unchanged.

| Include | Additional data |
| --- | --- |
| `base` | Version, server identity, platform/context, listen port, last tick, hubs/capabilities, settings, light breaker and active mode |
| `controls` | Complete set of rooms and standalone lights, plus per-room light/button/motion/contact counts |
| `nodes` | Complete public node set, including child devices |
| `configuration` | Active profile/effective values, location/solar data, full mode configuration, transitions, profiles, scenes, input bindings and review summary |

The base is always included. `controls` and `nodes` are mutually exclusive;
unknown includes and include strings longer than 128 bytes return HTTP 400.
Examples:

```text
GET /api/state?include=base
GET /api/state?include=controls,configuration&authoritative=true
GET /api/state?include=nodes
GET /api/nodes/state?scope=controls
```

Every selected state response carries a receipt:

```json
{
  "state_scope": {
    "schema_version": 1,
    "included": ["base", "controls", "configuration"],
    "nodes": "controls"
  }
}
```

`nodes` is `none`, `controls`, or `all`. An omitted node section leaves the cache
unchanged; a successful empty array replaces the complete requested subset.
Replacing controls preserves cached children of retained rooms, and removes
children of deleted rooms. A full empty node response clears the node cache.
Malformed or contradictory receipts fail without replacing the cache.

`authoritative=true` always runs the existing observed-power reconciliation
before building the response, including for base-only requests. Includes do
not restrict reconciliation or change command/observation authority. When
configuration is requested, the global rhythm interval still accounts for
every node's effective settings. Unrequested nodes skip per-node DTO and
lighting-display construction; base-only reads also skip configuration work.

The SDK requests controls and configuration together during connect/resume.
The server advertises `state_includes_v1`; a selected hello must carry both
that capability and the requested scope. Older servers ignore the query and
return a full snapshot, which the SDK continues to accept. Older apps and
admin/debug/backup consumers retain unqualified full reads. The app and
appliance may roll back independently; no persistent schema changes.

On capable servers, periodic node polling uses `scope=controls`. Omitted scope
or `scope=all` retains the full poll; unknown scopes return HTTP 400. Live SSE
events remain unchanged. Opening a room's Bulbs, Motion or Buttons tab, a
device sheet, Wake/Sleep Button settings, or hub details explicitly fetches the
full node catalog and topology. These requests are coalesced, expose loading/failure/retry, and apply
only to the same server and connection generation. Live events received during
the fetch are replayed in arrival order after applying the snapshot. A
visible device surface opts into full fallback polling until its last consumer
closes; normal polling then returns to controls. A new hello invalidates detail
freshness; it does not trigger a background catalog fetch. Finer per-room/device selection is outside this first contract.

Diagnostics record response bytes and build duration without node payloads.
The existing `app_control_readiness` PostHog event adds the bounded `state_scope`
property. Read selection creates no new Supabase light-activity records; action
acceptance, observations, delivery and retention remain unchanged. Compare
readiness timings and payload/node buckets by scope, version and transport
after deploying both sides. Synthetic payload tests establish response-size
reduction, not a measured appliance latency improvement.

Acceptance coverage lives in `scenario_selective_state`,
`scenario_authoritative_state_refresh`, the SDK state-includes/scope tests,
and Flutter `selective_state_test.dart` plus startup snapshot tests.
