# Move a Rhythm Matter fabric to Home Assistant's matter.js server

This offline script prepares **native matter.js storage** from a complete Rhythm
Matter backup. It preserves the fabric's CA, fabric ID, controller node ID, CASE
authenticated tags and original IPK, and imports the commissioned-node inventory.
Devices should not need factory resets, sharing codes or re-pairing.

The destination is **OHF `matter-server` 1.5.5 with matter.js 0.18.0**, not Python
Matter Server. These are the embedded server/library versions, **not** the Home
Assistant Core or Matter app version. The script refuses other versions. Support
for another release requires updating the adapter and running its tests; changing
the version check alone is insufficient.

Automated tests cover offline conversion and the real target controller restoring
the fabric and peers over two restarts on a simulated network. **Physical-device
takeover and HAOS installation have not yet been qualified.** Treat the output as
a prepared migration requiring a controlled cutover, not confirmation of working
devices. Start with a fresh HA Matter server; existing fabrics cannot be merged.

## Prepare

Use Linux or macOS with Node.js 22.13+ (Node 24 recommended) and npm. From this
directory in a checkout of the public Rhythm OS repository:

```sh
npm ci --ignore-scripts
node migrate.mjs --help
```

Choose one source:

- A **full-secret Rhythm backup JSON** (schemas 1–3) containing
  `installation.integration_files`. A portable lighting-settings export or a
  redacted support bundle cannot restore a Matter fabric.
- A **cold copy of the installation data directory**, containing
  `matter/fabric-identity.json`, `matter/chip/devices.json` and exactly one CHIP
  controller store. Stop the Rhythm appliance/controller before taking this
  copy; do not point the converter at actively changing live data.

Normal native storage is
`matter/chip/chip_tool_config.controller-storage.ini`, even though the controller
was configured with a `.json` path. Earlier CHIP INI/JSON filenames are recognized
explicitly. Multiple candidate stores fail rather than guessing which owns the
devices. Retain the original source, appliance image and recovery backup.

```sh
node migrate.mjs --source /private/rhythm-backup.json \
  --target-version 1.5.5 --dry-run

node migrate.mjs --source /private/rhythm-backup.json \
  --target-version 1.5.5 --output /private/ha-matter-import

node migrate.mjs --verify /private/ha-matter-import
```

The output parent must exist; the output directory itself **must not exist** and
must be outside the source snapshot. Dry-run validates credentials and IDs without
creating an output directory (a private temporary parser file is removed). The
script performs no network requests, commissions no devices, stops no services
and never edits the source. It does not accept URLs or cloud credentials.

Output:

| Path | Purpose |
| --- | --- |
| `matter-server/` | Native WAL/directory storage to give the target server |
| `receipt.json` | Version, source digest, exact IDs and checksums; no private keys |
| `device-map.json` | Fabric-scoped node/endpoint inventory for later HA mapping |
| `PREPARED` | All preparation steps completed |

Output directories use mode 0700 and files 0600. **The native storage contains the
fabric's private keys.** Keep the entire output private and out of Git, tickets and
chat. The tool prints only bounded counts/status and validation errors.

An interrupted/failed attempt retains `INCOMPLETE`; do not use it. Inspect or
remove it yourself and rerun to a different new directory. There is no `--force`
or automatic cleanup of prior output. `--verify` validates the original prepared
storage and device map, including after copying them to another machine. Normal
server startup rotates keys and changes storage, so these checksums are only for
**before first startup**, not a runtime health check.

## Cut over to Home Assistant

1. Check the **embedded OHF Matter Server version** on the destination. Use 1.5.5;
   an HA app version such as 9.x is not the required version number. Preserve an
   HA cold backup even if its Matter server has no paired devices. Stop that
   server before installing data.
2. Stop the original Rhythm Matter controller and prevent it from restarting.
   Do not run two copies of this controller identity simultaneously. Leave
   devices and their network credentials unchanged. For Thread, keep a working
   border router; fabric conversion does not migrate a Thread dataset.
3. Copy the **contents of `matter-server/`** into a fresh target storage directory,
   preserving all files and assigning ownership to the account running the
   server. Do not copy the entire rpiz data directory. Do not combine this with
   another controller's `config` or `server-*` directories. Preserve destination
   originals rather than deleting them.
4. Start the server with the usual HA namespace selection: `--fabricid 2
   --vendorid 4939`. The prepared namespace is `server-2-134b`; those arguments
   select the storage location. The **actual imported fabric ID and vendor ID
   remain the original signed values**, including full-width u64 IDs. Do not
   substitute the original fabric ID on the CLI or rename that directory.
5. Connect HA's Matter integration to that server, wait for fresh interviews,
   and verify every expected node, endpoint, state update and light command.
   Restart the Matter server and confirm that they return. A receipt or cached
   device listing is not proof of device access. Leave new pairing, OTA, fabric
   removal and network changes until the takeover has been verified.
6. In Rhythm's HA add-on, transfer portable lighting settings and explicitly map
   and approve the HA lights. See [the HA migration guide](../../docs/home-assistant-migration.md).
   `device-map.json` deliberately leaves `ha_entity_id` unset; physical identity
   and endpoint evidence must establish those matches. Native groups/scenes,
   button bindings and Thread infrastructure need their own checks.

For HAOS, the storage directory belongs to the Matter app, **not** HA's ordinary
`/config` directory. Access and the container's writable-volume location depend
on the installed app. This script deliberately has no guessed Supervisor path
or live-install command. If the app embeds a different server version, do not
install this data there. A separately hosted, pinned server is an alternative:
install `matter-server@1.5.5` on the target Linux host and run
`matter-server --storage-path /private/ha-matter-import/matter-server --fabricid 2 --vendorid 4939`.
Configure HA's Matter integration with that server's WebSocket URL. Keep port 5580
on a trusted LAN; the Matter Server API must not be exposed publicly.

Rollback before further commissioning or fabric/network changes: stop the HA
Matter server and Rhythm add-on, then restart the original controller from its
matching cold data snapshot and image. Never delete the fabric from the devices
as part of rollback. Once destination state has evolved, use its own backup and
review recovery rather than reverting blindly to a stale source.

## Validation and maintenance

```sh
npm test
```

Tests generate synthetic CA/ICA keys and inventories; there are no customer
fixtures. They exercise full-secret JSON and cold-directory input, legacy node
arrays, exact large numeric IDs, original IPK consistency, root-only and
intermediate chains, private-key mismatch, duplicate/malformed input, permissions,
secret-safe CLI failures, exclusive output creation and native controller restart.

The version-pinned adapter reuses OHF's certificate reader and peer injector but
does not feed its Python migration loader. That loader substitutes a fixed IPK
and uses lossy `number` conversions in fabric selection. The script supplies the
original IPK, preserves IDs as `bigint`, checks the derived operational IPK, and
issues a replacement NOC under the original CA. It uses native storage drivers,
never hand-edits a live server database. Sessions are renegotiated; session
resumption state and native group counters/keys are not copied.

Any library/storage version update must rerun the real controller restore tests
and qualify live takeover before claiming broader compatibility. Merely matching
the Matter protocol version does not establish storage compatibility.
