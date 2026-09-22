# Saved networks: Box connection, accessory provisioning and Matter Wi-Fi changes

The Box owns one catalog of up to 16 saved networks. Three consumers share it
through two independent pointers:

| Pointer | Meaning | Written by |
| --- | --- | --- |
| `box_profile_id` | The network the Box itself joins and restores at startup | Only the Box connection path (BLE/API provisioning, startup sync, forget) |
| `default_id` | The network new accessories receive during setup | Only the owner, in **Settings → Wi-Fi for new accessories** |

Matter network changes name a profile explicitly. Pairing never asks for a
network inline: every provisioning journey uses `default_id`. The API still
accepts an optional `wifi_profile_id` on Box BLE commissioning for tooling.

Choosing a default never moves the Box, and moving the Box never discards saved
networks. When the Box joins a network, that network is upserted by SSID and
`box_profile_id` moves to it. A default that was following the Box (unset, or
equal to the previous Box network) keeps following it; an owner-chosen default
stays. If the catalog is full, the new connection cannot be stored, so
`box_profile_id` is cleared instead: startup recovery then uses the system network
configuration and never rejoins the previous network. Forgetting the Box network
clears only the pointer. The Box's **Change Wi-Fi** dialog lists the other saved
networks and moves the Box by profile ID, so a saved password is never re-typed or
sent by the app; a new network is still typed, and joins the catalog once the Box
connects. The owner catalog cannot edit or remove the Box
connection, cannot remove the default while another network exists (the Box never
chooses where accessories go), and stores one entry per SSID.

`commissioning_wifi.json` is an atomic, durable mode-0600 secret document owned by
storage, which serializes every read-modify-write under its own lock. The legacy
top-level `ssid` and `password` mirror the Box network, or the default when no Box
network is known; `profile_schema: 1`, `revision`, `default_id`, `box_profile_id`
and `profiles` carry the catalog. Older software restores the Box from that
mirror, so a downgrade keeps the Box on its own network. A legacy credential was
both roles and seeds one profile holding both pointers. A managed empty catalog
stays empty. An older credential writer replaces the catalog with one credential,
so other networks require re-entry after that downgrade.

Owner-entered networks must be provisionable (open, 8–63 byte passphrase, or
64-digit key). The Box connection is recorded as joined, so stored entries are
checked only structurally. An unreadable or self-inconsistent document is moved to
`commissioning_wifi.json.corrupt` and the platform seeds a working catalog again.
A document with a newer `profile_schema` is never reinterpreted, replaced or
quarantined: the catalog is unavailable, and the Box and accessories continue from
its legacy mirror. Old and current factory reset erase the document and its
quarantine.

Neither backup mode exports saved networks. Restoring a backup retains the local
Box's profiles; another Box needs credential re-entry. Profiles never enter normal
state snapshots, debug bundles or telemetry. Catalog and credential routes require
an owner token even on LAN, deny support tokens, and return `Cache-Control: no-store`.

| Route | Contract |
| --- | --- |
| GET `/api/pairing/wifi-profiles` | Metadata only: revision, `default_id`, `box_profile_id`, profile IDs and SSIDs |
| PUT `/api/pairing/wifi-profiles` | Revision, UUID correlation ID, `save`/`remove`/`default`, optional profile ID; omitted password preserves it, empty password selects an open network |
| GET `/api/pairing/wifi-profiles/:id/credentials` | Explicit uncached owner credential retrieval for phone BLE |
| PUT `/api/wifi/profile/:id` (appliance) | Move the Box to a saved network. Owner-only on LAN and remotely; the password never leaves the Box. Same accepted/scheduled contract as PUT `/api/wifi`; an unknown profile is 404 with no detail |
| GET `/api/wifi` (appliance) | Adds `last_change`: `{ssid, state}` with `running`/`succeeded`/`failed`, or null. `failed` is recorded only after the Box is back on its previous network, so the app can show that a move was rejected. Memory only |
| POST `/api/wifi/verify` (appliance) | UUID operation ID, SSID and password. The Box scans, joins the network for up to 30 s, then always returns to its own. 200 with `state` `running` or `passed` (already the Box network); 409 when the Box has no Wi-Fi of its own or the radio is busy. Nothing is saved |
| GET `/api/wifi/verify/:operation_id` (appliance) | `state` `running`/`passed`/`failed`, with `reason` `not_found` or `join_failed`. Held in memory only; 404 after a restart. Clients treat 404/409 as "cannot check" and still allow the save |
| GET `/api/pairing/wifi-credentials` | Credentials of `default_id` for phone BLE provisioning |
| POST `/api/matter/wifi-change` | UUID operation ID, saved profile ID, registered native Matter device ID; returns a receipt |
| GET `/api/matter/wifi-change/:id` | Same receipt across retries and app reconnect |
| GET `/api/matter/wifi-change?device_id=...` | Latest device receipt for reopening the screen after app restart |

New clients require `saved_wifi_profiles_v1` and `matter_wifi_change_v1`. Existing
pairing requests remain valid; an optional `wifi_profile_id` selects a profile.
The API reports stale catalog edits as 409. Missing profiles, owner failures and
uncertain transport delivery stay distinct in the SDK.

Only one network change runs at a time per appliance; the admission fence lives
in `AppState`, not in process globals. Native preflight checks authenticated
reachability, a Wi-Fi Network Commissioning interface and a direct light endpoint.
The transaction arms a 180-second device fail-safe and stages everything under it:

1. **Make room.** Most shipping bulbs hold one network. When no slot is free, the
   connected network is staged for removal. The bulb stays on it until Connect,
   and rollback or fail-safe expiry restores it. With a free slot nothing is removed.
2. **Add, then Connect**, each with typed status.
3. **Verify the target**: expire sessions for the node, establish CASE again, and
   read the connected network plus the light's OnOff attribute. A bulb moving
   networks over its operational session commonly cannot deliver the Connect
   response, so a lost response is not treated as failure; only this authenticated
   readback decides. A typed rejection still rolls back without it.
4. **Commit** with typed CommissioningComplete and verify again.

No fabric, node, room, group, name or schedule writer participates.

Failure attempts fail-safe rollback and reports separately whether the old network
was verified. A lost completion response remains recovery-required. Transport
failure never replays the RPC. Durable receipts contain no credentials, retain a
five-minute recovery fence, and resolve an interrupted process to recovery-required
without replay. The remaining fence travels to the native command owner as a
relative budget measured on monotonic clocks, including time queued behind another
controller lifecycle operation, so neither delayed work nor a wall-clock step can
arm a fail-safe that extends beyond the fence. The controller RPC waits for that
budget plus a 30-second readback margin. Reopening the app reads the latest receipt. Unknown receipt status
is not permission to repeat a request. Receipts are limited to 256 within 30 days;
backup export excludes them and imports reject them. Reset/restore waits for an
active operation or recovery fence to finish.

The supported first slice requires the old and target networks to remain available
and reachable from the Box. Thread and bridged devices are rejected. An offline bulb may need its old network restored or a
manufacturer-supported recovery procedure. Verify one-slot replacement, a lost Connect response, credential rejection,
absent AP, rollback, power loss, reboot persistence and ordinary controls on representative
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
