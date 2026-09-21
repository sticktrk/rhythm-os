# Home Assistant deployment

The Home Assistant build combines `rhythm-addon`, the pure Dart `admin-api/bin/local_server.dart` composition root, and `admin-ui/local.html`. Container and repository metadata live in [rhythm-home-assistant](https://github.com/sticktrk/rhythm-home-assistant). Flutter remains the mobile application and is not built or served by this deployment.

## Authority and isolation

The Rust integration registry contains only Home Assistant. Startup requires the current Supervisor credential and constructs exactly one internal `supervisor:80` connection. Persisted credentials contain a `credential_source: supervisor` marker, never the Supervisor token. Location and timezone refresh from Home Assistant on connect/sync. HA owns devices, integrations and areas; the add-on has no credential, room rename, pairing, OTA, remote tunnel or full device import routes.

The Ingress gateway forwards only Supervisor-authenticated identity headers to the loopback Dart API. The API verifies that the identified HA user is active and is an owner or administrator using HA's authenticated WebSocket API. Every write rechecks membership; reads cache it for at most 15 seconds. Session responses contain no service token. Writes require same-origin browser headers, a request ID and a current server identity. Curve writes retain the shared canonical configuration hash precondition. No write is retried after an uncertain response.

Rust binds loopback and requires a random owner token supplied by the container through `/run/rhythm/api-token`. The add-on middleware separately requires positive token verification, including during factory reset, and denies routes outside its operation catalog. The staff API/UI entry points remain separate. Shared editors use an injected local DeviceClient; no cloud hub identity is invented.

## Persistence and lighting safety

All state is under `/data/rhythm`; `/data/options.json` belongs to Supervisor. New installations start paused with an empty managed-light selection. Saving a selection requires pausing first and compares the reviewed previous selection before a durable atomic write. HA area commands resolve to explicit selected entities; newly discovered lights receive no commands until selected. Empty, missing or reset policy fails closed. Entity IDs are the v1 selection key: renames require review, and reusing an old HA entity ID for different hardware requires deselecting it before replacement.

Home Assistant cold backups include the full `/data` volume and selection. Profile export/import transfers only the profile bundle; imports pause first and do not restore integrations or ownership selection. A Rhythm factory reset also clears the managed selection, profiles, bindings and credentials before the container restarts. Startup recreates the internal HA connection and ephemeral owner credential. Cloud upload and remote access configurations are cleared for this deployment. No cloud analytics connection is provisioned; existing local activity and request correlation remain available.

## Build and validation

Run `cargo test -p rhythm-addon -p rhythm-ha --lib --bins` and `cargo test -p rhythm-os --lib` for runtime changes. In `admin-api`, run `dart analyze` and `dart test`; in `admin-ui`, run both `npm run build` and `npm run build:homeassistant`. The latter writes only the local entry point to `dist-homeassistant`. Public package builds consume an immutable product revision.

The initial release is experimental. Container-level checks do not replace actual HAOS qualification of administrator authorization, revoked sessions, nested Ingress navigation, Core/Supervisor restarts, both CPU architectures, backup/restore, interrupted upgrades and resource use. The new repository has a different Supervisor installation identity from the legacy repository. Do not run both controllers on the same lights. Follow the packaging repository's migration instructions; cross-install full-backup import is deliberately unavailable.

## Follow-up qualification

Automatic registry/config event reconciliation beyond existing reconnect and manual sync, a richer migration preview, theme alignment with HA, and measured long-running resource budgets remain separate qualification work. A manual sync is available for changes made while connected. These limitations must remain visible until tested improvements land; this source change alone does not claim a production release.
