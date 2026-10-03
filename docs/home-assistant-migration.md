# Moving device ownership to Home Assistant

The add-on runs the Rhythm lighting engine and shared API. Flutter connects to the
mobile listener over LAN or the existing Rhythm tunnel. The React administration
interface remains behind Home Assistant Ingress. HA owns device networking,
commissioning, integrations, areas and physical observations.

## Client operation contract

Read `capabilities.deployment` on `/api/state` before presenting deployment-specific
operations. Absence means the existing appliance compatibility contract; an explicit
HA deployment does not support whole-appliance backup or restore.

| Operation | Mobile | HA administration | Authority |
| --- | --- | --- | --- |
| Rooms, profiles, curves, scenes, schedules, modes, manual lighting | Shared Rhythm API | Shared Rhythm API | Rhythm desired behavior |
| State and events | Authenticated REST/SSE | Admin API with refreshed state | HA observations and Rhythm runtime |
| Phone enrollment | Exchange single-use code | Generate code after fresh HA admin authorization | Rhythm durable installation and bearer credentials |
| LAN and remote access | Direct API and existing Rhythm tunnel | Credential revocation | Rhythm account/home and tunnel contracts |
| Managed lights | Deployment capability | Reviewed selection while paused | HA registry identity plus explicit Rhythm opt-in |
| Physical device setup | Hand off to HA | Manage in HA | HA integration |
| Portable behavior | Profile bundle | Profile import/export | Rhythm; import pauses control |
| Whole installation recovery | Unavailable through appliance backup APIs | HA cold backup | HA Supervisor |
| Appliance OTA, network provisioning, direct protocol pairing | Unavailable on add-on | Unavailable | HA host/app lifecycle |

The internal admin credential and Supervisor token are never phone credentials.
Ingress identity headers have no authority on the mobile listener. The mobile
listener requires positive bearer authorization even when the phone is on LAN.
Unknown operations remain denied. Old appliances retain their current routes.

## Offline migration preview

Export the old Rhythm installation's backup and the new add-on's authenticated
`GET /api/devices/canonical` catalog to private local files. Retain the old image
and its matching data snapshot; neither file belongs in source control. Run from
the public product root:

```sh
cargo run -p rhythm-os --example ha-migration-preview -- \
  /private/old-backup.json /private/ha-catalog.json /private/new-review-directory
```

The converter accepts backup schemas 1–3, bounds input size, and exclusively
creates a private output directory. `review.json` records a deterministic input
digest, every old device, exact hardware candidates, migration methods and pending
reviews. `profiles.json` can be imported through the add-on's System page. It
contains typed portable profiles, schedules and user scenes; integration-native
scenes and opaque scene extensions are excluded and listed for review. It never
copies credentials, fabrics, connectors, ownership selection or live mode state.
Identical inputs produce identical review content; an existing output directory is
never overwritten, including after interruption.

A hardware match remains a candidate. Names and room names are never identity
proof. Matter node IDs are fabric-scoped and cannot match across controllers.
Serial matches additionally require manufacturer and model. Multiple HA endpoints,
unavailable counterparts and device-specific behavior require explicit review.
Room preferences, layout aliases, input bindings, schedule references and native
scene equivalents need a reviewed mapping before enabling control; the preview
does not claim to apply these installation-specific mappings.

## Device handover

1. Preserve the old image, cold data backup, setup information and rollback plan.
2. Establish the exact device model's HA integration. For Matter, retain the old
   controller while using its supported multi-admin window. Confirm Thread
   connectivity separately; adding a fabric does not migrate a Thread network.
3. Verify fresh HA observations and control of each candidate, including inputs,
   colors, transitions and scenes. Unsupported models stay on the old installation.
4. Import portable behavior while paused, review HA identities, assign rooms and
   inputs, and approve the managed-light selection. Re-enroll phones and perform an
   explicit account/tunnel handover; copying an appliance data directory is unsafe.
5. Stop the old lighting writer before enabling the add-on for those same lights.
6. Keep old recovery material through the agreed transition window. Before rollback,
   stop the new writer and connector, then restore the matching old image **and**
   data snapshot. Do not restore only an older image against new live stores.

HA backups can contain phone and tunnel secrets. A replacement-host restore must
stop/revoke the previous connector and reconcile account/installation ownership
before remote access is enabled. A copied backup is not authorization to run two
active installations. Automated cross-host identity handover is not implemented by
this preview. Reset clears Rhythm state; it must not remove HA devices or options.

## Qualification and retirement gates

Deterministic unit and fake-I/O scenarios are necessary, but do not establish real
HAOS behavior or device support. Record exact HA Core, OS, Supervisor, Matter-app,
mobile and product versions for the release candidate. Broad historical version
series are insufficient compatibility evidence.

The release evidence must cover native amd64 and aarch64 images, HAOS Ingress and
custom host-port mapping, real phones on Wi-Fi and cellular, tunnel disable/recovery,
token revocation, two-interface convergence, registry rename/replacement, Core
outage, cold restore, interrupted upgrade, rollback/forward recovery, duplicate
installation prevention and a sustained resource/reconnect run. Device pilots must
cover each supported category; proprietary BLE support is not implied by HA's
Bluetooth support. ARMv6 Pi Zero appliances require a supported HA host.

Until those gates pass, legacy runtimes and commissioning dependencies remain as
migration/rollback support. The add-on's production dependency graph must stay HA
only. Delete the legacy Matter/CHIP, BLE/vendor stacks, native phone commissioners,
appliance installers and obsolete release jobs only after successful pilot and
rollback qualification. Do not present this source migration as a qualified release.
