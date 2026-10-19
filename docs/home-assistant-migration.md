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
| Matter setup and original-code recovery | Capability-gated HA-backed app flow | Manage in HA | HA fabric; private original-code retention in Rhythm |
| Other physical device setup | Hand off to HA | Manage in HA | HA integration |
| Portable behavior | Account lighting-settings backup and reviewed restore | Profile import/export | Rhythm; import pauses control |
| Whole installation recovery | Unavailable through appliance backup APIs | HA cold backup | HA Supervisor |
| Appliance OTA, network provisioning, direct protocol pairing | Unavailable on add-on | Unavailable | HA host/app lifecycle |

The internal admin credential and Supervisor token are never phone credentials.
Ingress identity headers have no authority on the mobile listener. The mobile
listener requires positive bearer authorization even when the phone is on LAN.
Unknown operations remain denied. Old appliances retain their current routes.
The native app retains known add-on ownership across reconnects and suppresses
saved direct hub connections while that deployment is selected. An rpiz remains
on its existing device path. See [Matter management and deferred parity](home-assistant-addon.md#matter-management-from-the-mobile-app).

## Transfer lighting settings with the app

Use **Settings → Rhythm App → Backup & Restore** in the Rhythm app to move
lighting behavior from a Rhythm Box to the add-on. The app and destination must
support portable lighting settings. An older add-on shows **Update add-on to
transfer settings**; older appliance account backups remain usable as the source.

1. Connect the app to the old Rhythm Box, sign in, and choose **Back Up Now**.
   Keep the old installation and its recovery backup until the move is verified.
2. Add the physical lights to Home Assistant using their supported integrations.
   In the add-on, use **Connect mobile app** to enroll the phone, then connect the
   app to the add-on.
3. Open **Backup & Restore → Restore From Backup**. If the account also has
   saved add-on settings, **Choose a settings backup** lets you choose between
   those settings and the old Light Box backup.
4. In **Match your rooms and lights**, select the destination for each old room
   or light you want to transfer. Every mapping starts at **Skip for now**; verify
   each match explicitly. Leave devices not yet in Home Assistant unmapped.
5. Choose **Restore settings**. Rhythm imports portable profiles, schedules,
   user scenes, mode configuration and mapped room/light preferences. The review
   reports skipped nodes and exclusions. It pauses adaptation for review and
   does not grant permission to control any light.
6. Review the resulting settings, select verified identities under **Managed
   lights**, and stop the old controller before enabling Rhythm for those lights.
   Lights added to HA later can receive their old settings through another
   reviewed transfer.

The transfer excludes appliance credentials, Matter fabrics, hub connections,
tunnel identity, active output state and managed-light ownership. Home Assistant
remains responsible for devices, areas and inputs. Integration-native scenes and
opaque scene extensions are excluded; recreate their equivalents and input
bindings in the destination as needed. Re-pairing or adding a Matter device to
another controller remains a separate device setup step.

The account retains portable lighting settings separately from its appliance
rollback bundle. Subsequent add-on backups can update lighting settings without
replacing that old full backup or its source metadata. Sign-in alone does not
migrate settings. App layout remains account data; source room IDs and
input/device references do not establish destination identity. A lighting
settings backup is not a full Home Assistant installation backup.

The API advertises `capabilities.deployment.portable_settings`. Supporting clients
export `GET /api/lighting-settings`, convert a source bundle with
`POST /api/lighting-settings/preview`, and apply settings with explicit reviewed
target node mappings through `PUT /api/lighting-settings`. Missing capability support does
not authorize falling back to full appliance restore on the add-on. The existing
profile-only bundle API and offline converter remain available.

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
reviews. `profiles.json` can be imported through **System → Restore profiles**.
The System page accepts both this standard `profile_bundle` file and its own
`rhythm-ha-profiles` export wrapper. It contains typed portable profiles,
schedules and user scenes; integration-native
scenes and opaque scene extensions are excluded and listed for review. It never
copies credentials, fabrics, connectors, ownership selection or live mode state.
Identical inputs produce identical review content; an existing output directory is
never overwritten, including after interruption.

A hardware match remains a candidate. Names and room names are never identity
proof. Matter node IDs are fabric-scoped and cannot match across controllers.
Serial matches additionally require manufacturer and model. Multiple HA endpoints,
unavailable counterparts and device-specific behavior require explicit review.
This offline profile-only preview does not apply room/light preferences, layout
aliases or input bindings. Use the app's reviewed lighting-settings transfer for
room/light preferences; review schedule references and native scene equivalents
before enabling control.

## Device handover

1. Preserve the old image, cold data backup, setup information and rollback plan.
2. Establish the exact device model's HA integration. For Matter, retain the old
   controller while using its supported multi-admin window. Confirm Thread
   connectivity separately; adding a fabric does not migrate a Thread network.
3. Verify fresh HA observations and control of each candidate, including inputs,
   colors, transitions and scenes. Unsupported models stay on the old installation.
4. Transfer portable behavior with reviewed room/light mappings while paused,
   review HA identities and inputs, and approve the managed-light selection.
   Re-enroll phones and perform an
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

The app transfer and profile-file compatibility changes require real RPiZ-to-HA
verification on the released app and add-on builds. Automated tests do not prove
that a particular physical device can be transferred or that its HA integration
supports the same lighting behavior.

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
