# Saved provisioning networks and Matter Wi-Fi changes

The Box owns up to 16 saved networks and exactly one default while the catalog is
nonempty. The owner manages these in **Saved provisioning networks**. Provisioning
can select another profile for one attempt without changing the default or the
Box connection. Box Matter and Box/phone BLE Wi-Fi provisioning use this selection.
Phone OS Matter commissioning controls its own network; an explicit saved-network
selection cannot silently fall back to that path.

`commissioning_wifi.json` is an atomic, durable mode-0600 secret document. The
legacy top-level `ssid` and `password` represent the default; `profile_schema: 1`,
`revision`, `default_id`, and `profiles` carry the catalog. Legacy credentials seed
one profile. A managed empty catalog stays empty. New Box network writes preserve
managed profiles. Older software can read the default; an older credential writer
replaces the catalog with one credential, so alternate profiles require re-entry
after that downgrade. Old and current factory reset erase the entire file. There
is no separate credential file to resurrect after rollback.

Neither backup mode exports saved networks. Restoring a backup retains the local
Box's profiles; another Box needs credential re-entry. Profiles never enter normal
state snapshots, debug bundles or telemetry. Catalog and credential routes require
an owner token even on LAN, deny support tokens, and return `Cache-Control: no-store`.

| Route | Contract |
| --- | --- |
| GET `/api/pairing/wifi-profiles` | Metadata only: revision, default ID, profile IDs and SSIDs |
| PUT `/api/pairing/wifi-profiles` | Revision, UUID correlation ID, `save`/`remove`/`default`, optional profile ID; omitted password preserves it, empty password selects an open network |
| GET `/api/pairing/wifi-profiles/:id/credentials` | Explicit uncached owner credential retrieval for phone BLE |
| GET `/api/pairing/wifi-credentials` | Legacy default credential retrieval |
| POST `/api/matter/wifi-change` | UUID operation ID, saved profile ID, registered native Matter device ID; returns a receipt |
| GET `/api/matter/wifi-change/:id` | Same receipt across retries and app reconnect |
| GET `/api/matter/wifi-change?device_id=...` | Latest device receipt for reopening the screen after app restart |

New clients require `saved_wifi_profiles_v1` and `matter_wifi_change_v1`. Existing
pairing requests remain valid; an optional `wifi_profile_id` selects a profile.
The API reports stale catalog edits as 409. Missing profiles, owner failures and
uncertain transport delivery stay distinct in the SDK.

Only one network change runs at a time. Native preflight checks authenticated
reachability, a Wi-Fi Network Commissioning interface, a direct light endpoint and
available network slots. It never removes a working network to free a slot. The
transaction arms a 180-second device fail-safe, checks typed Add/Connect status,
expires sessions for that node, establishes CASE again, and reads the connected
network plus the light's OnOff attribute. It commits with typed
CommissioningComplete and verifies again. No fabric, node, room, group, name or
schedule writer participates.

Failure attempts fail-safe rollback and reports separately whether the old network
was verified. A lost completion response remains recovery-required. Transport
failure never replays the RPC. Durable receipts contain no credentials, retain a
five-minute recovery fence, and resolve an interrupted process to recovery-required
without replay. The receipt deadline reaches the native command owner; delayed
work cannot arm a fail-safe that extends beyond that fence. Reopening the app reads the latest receipt. Unknown receipt status
is not permission to repeat a request. Receipts are limited to 256 within 30 days;
backup export excludes them and imports reject them. Reset/restore waits for an
active operation or recovery fence to finish.

The supported first slice requires the old and target networks to remain available
and reachable from the Box. Thread, bridged devices and devices without a spare
slot are rejected. An offline bulb may need its old network restored or a
manufacturer-supported recovery procedure. Verify credential rejection, absent AP,
rollback, power loss, reboot persistence and ordinary controls on representative
physical bulbs before publishing model-level reliability claims. Native header
compilation and deterministic transaction tests do not substitute for those tests.

App events use `wifi_profile_action` and `matter_wifi_change` with bounded
`action`/`outcome` plus an opaque journey UUID. Rust outcomes extend the existing
`server_device_lifecycle_events` authenticated ingestion path with `wifi_profile`
and `wifi_change`; the existing bounded local history, event deduplication, RLS and
owner deletion cascades apply. No network name, credential, profile/device ID or
raw error is included. Deploy the additive database action constraint and ingestion
allowlist before appliance/chipd, then the app. No release is performed by this
change.
