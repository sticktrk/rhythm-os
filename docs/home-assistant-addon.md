# Home Assistant deployment

The Home Assistant build combines `rhythm-addon`, the Dart
`admin-api/bin/local_server.dart` composition root, React `admin-ui/local.html`
and cloudflared. Container metadata lives in
[rhythm-home-assistant](https://github.com/sticktrk/rhythm-home-assistant).
Flutter remains a separate first-class mobile client. See the
[operation and migration contract](home-assistant-migration.md).

## Listeners and credentials

The mobile API binds port **54448**, mapped by Supervisor to a configurable host
port. Phones use the HA host's reachable LAN address and that mapped port.
The existing Rhythm tunnel keeps its `http://localhost:54448` origin inside the
container. Host port changes do not change the tunnel origin. The child-process
controller owns cloudflared; HA owns the container lifecycle.

The child controller retains at most two 1 MiB connector log files, including
while a connector runs continuously. Log storage failure does not block its
output pipe. Repeated short-lived exits use exponential retry delays from two
seconds to one minute. A run lasting a minute or a new configuration resets that
delay. The image supplies a pinned cloudflared binary with runtime auto-update
disabled; connector updates arrive through a new tested HA image.

The internal Rust administration API binds **127.0.0.1:54449**. The Dart API
binds **127.0.0.1:8787**, and only the trusted Ingress gateway can reach it.
The gateway forwards Supervisor-authenticated identity headers. Dart verifies
that the HA user is active and an owner or administrator through HA's WebSocket
API. Writes recheck membership; reads cache it for at most 15 seconds. Browser
writes require same-origin headers, a request ID and current server identity.
Curve writes preserve canonical configuration hash preconditions. No uncertain
write is automatically retried.

The ephemeral admin credential is held outside the durable phone auth store.
Neither phone tokens nor forged HA identity headers authorize the other listener.
The mobile listener requires a bearer token on LAN and through the tunnel;
appliance open-LAN claiming is unavailable. An HA administrator uses **Connect
mobile app** to issue a five-minute, single-use, installation-bound code, then
enters it in the phone's add-server flow. New codes replace pending codes, and
restart/reset discards them. Revocation applies to REST and active SSE streams.
Supervisor and cloudflared credentials stay server-side.

## HA device ownership

The runtime registers only `rhythm-ha`, constructing one Supervisor connection.
The persisted credential contains a `credential_source: supervisor` marker,
never the Supervisor token. HA owns physical integration setup, areas, location
and timezone. Direct device pairing, Wi-Fi provisioning, appliance OTA and full
appliance backup import/export remain denied by an explicit route policy.

New installs start paused with empty selection. Reviewed registry identity is
separate from mutable routing entity IDs. Selection requires a current snapshot
revision and previous-selection precondition. New, replaced, disabled or
unresolved devices do not inherit control. Commands resolve to exact selected
entities, including area commands. Complete inventory refreshes replace the
catalog; partial failures retain previous state without authorizing stale writes.

## Persistence and recovery

Rhythm state lives under `/data/rhythm`; Supervisor owns `/data/options.json`.
Phone credentials, server identity and tunnel configuration survive ordinary
restart. Explicit tunnel disablement is durable. Startup validates security stores
instead of silently clearing invalid state or substituting a new identity.
Cloud activity remains optional and is not provisioned by tunnel setup.

Legacy entity-ID selection is retained as rollback material and requires fresh
identity review for the new versioned selection. Restore the matching old image
and data snapshot together; old writers must not write new live stores. Profile
import pauses adaptation and carries no device ownership or integration secrets.
Mobile cloud snapshots use supported portable data for this deployment instead
of requesting a secret-bearing appliance backup.

HA cold backups include persistent Rhythm and tunnel secrets. Restoring to
replacement hardware requires explicit connector/account ownership handover;
a copied backup must not be enabled alongside the original installation. That
cross-host handover and full device migration remain qualification work. Reset
stops the connector and clears Rhythm state before restart, preserving HA devices,
integrations and Supervisor options.

Recovery pairs the immutable image digest with a cold backup of that
installation's complete `/data`. Record the add-on repository/slug, image build
identity, backup ID and retained backup hash before an upgrade. Restoring only
profiles does not restore credentials or managed-light ownership. A backup
created successfully is not evidence of a successful restore.

There is no single global data-schema version. Security stores currently use
auth and remote-access schema 1, server metadata schema 1 or 2, and reviewed HA
selection schema 2. Legacy entity-ID selection is preserved and sealed for
review, not silently granted authority. Unsupported security/selection schemas
fail closed without rewriting them. These guards do not promise that a prior
binary can read newer data: a rollback restores its matching image and cold data
snapshot, then reconnects to HA and revalidates selected identities before writes.

## Build and qualification

Run `cargo test -p rhythm-addon`, `cargo test -p rhythm-ha --features test-support`
and `cargo test -p rhythm-os --lib`. In `admin-api`, run `dart analyze` and
`dart test`; in `admin-ui`, run both `npm run build` and
`npm run build:homeassistant`. Run the SDK and affected Flutter tests, repository
invariants and the packaging validation/smoke harness. Public image builds consume
an immutable product revision.

`/health` and `/api/addon/status` include the same additive `build` object with
the product version, HA image version, product and packaging revisions, and
build-input hash. The local Dart health endpoint relays this public provenance.
An unpackaged build reports unknown image fields as null; its Cargo version must
not be mistaken for an HA image version. Health is process/API liveness; use the
connection and selection status for HA readiness. These responses contain no
installation credentials. Other deployment variants retain their health schema.

This remains an experimental deployment until the documented hardware and
migration gates pass. Synthetic Supervisor tests do not establish real HAOS
Ingress/port mapping, real phone LAN/cellular behavior, native amd64/aarch64
execution, supported physical models, cold restore, rollback or sustained resource
behavior. Legacy protocol/runtime removal follows those gates.
