# rpiz host flight recorder

`rhythm-host-recorder` is an rpiz-only diagnostic process supervised directly
by BusyBox init. It does not link to server state, RPC, HTTP, or application
logging. It discovers processes through `/proc` and treats application and
watchdog heartbeat files as optional evidence.

## Boot and failure-domain ordering

`S42hostrecorder` runs during `rcS`, after `/data` is mounted and before init
starts any `respawn` entry. It reads the new boot identity, prior restart
intent, pstore, watchdog state, and Raspberry Pi power/reset sources, archives
the old ring, and writes `early-boot.json`. Only after `rcS` finishes can init
start, independently and in this order:

1. `rhythm-host-recorder run --data-dir /data`;
2. `rhythm-hardware-watchdog` (the first process that opens `/dev/watchdog`);
3. `rhythm-launch` / `rhythm-server`.

Recorder failure cannot stop the watchdog or server. A missing/failed one-shot
does not block boot; the daemon performs a marked late fallback capture and
init respawns it after exit.

## Evidence schema and collection bounds

Records are compact schema-versioned NDJSON envelopes with a sequence, boot ID,
wall time, monotonic time, kind, and payload. Each observation carries one of:

- `ok` (including real zero or empty values);
- `unsupported`;
- `unavailable`;
- `permission_denied`;
- `timed_out`;
- `truncated` (with the bounded partial value when available).

Files are opened nonblocking and read under byte and elapsed-time limits. The
30-second summary contains CPU/load, selected memory/vmstat, PSI, diskstats,
`/data` capacity, R/D task counts, target process/thread counts, thermal and CPU
frequency readings, firmware power/throttle sources, watchdog state/time-left,
server/watchdog heartbeat ages, and recorder health. When a process leader is
in uninterruptible sleep, its bounded PID, name, and wait channel are retained
in the same summary that observed it. The 5-minute detail adds
bounded status and thread name/state/wchan for the server, CHIP daemon,
watchdog, and recorder.

Escalation occurs for a recorder cadence slip over 2 seconds, a D-state count
that persists for two samples, memory or I/O `full avg10 >= 1.0`, a
missing server after the 60-second startup grace, or a server heartbeat at
least 90 seconds old. A new signal escalates immediately, while the same signal
can repeat at most once per 15 minutes. It includes the blocked-task identity
from the triggering summary, bounded process I/O/status, at most four thread
stack/scheduler captures, and a 16 KiB tail from at most 256 KiB of `dmesg`
output collected by a child killed after 500 ms.

The collector never records environment variables, process argv/command lines,
credentials, network payloads, or user content.

## Ring, durability, and SD-card budget

Data lives below `/data/boot-diagnostics/host-flight-recorder/`. Current and
previous boots each use eight 248 KiB append-only segments. Rotation truncates
one old fixed-name segment; it never rewrites the whole ring. The two rings plus
their manifests stay below the strict 4 MiB combined budget. Early-boot pstore
uses a separate 256 KiB-per-boot bound.

Summary records have an 8 KiB total envelope cap, detail records 16 KiB, and
escalation records 64 KiB. The automated retention fixture writes 180 near-cap
summary records and 18 near-cap detail records and verifies the complete 90
minutes remains readable. A torn final line is ignored while every earlier
sequence-valid line remains usable.

Samples append into the kernel page cache. `fdatasync` occurs every 60 seconds,
on rotation/shutdown, and immediately after an anomaly; it does not run every
30 seconds. The default hard write envelope per day is:

- summary: `8 KiB * 2,880 = 22.5 MiB`;
- detail: `16 KiB * 288 = 4.5 MiB`;
- one continuously active escalation signal: `64 KiB * 96 = 6 MiB`;
- all six signal classes continuously active and staggered: at most `36 MiB`;
- manifest/sync updates: under about `3 MiB`, excluding filesystem metadata.

Thus the baseline schema-cap envelope is 27 MiB/day, and the deliberately
pessimistic all-sources-at-cap, all-signal ceiling is about 66 MiB/day plus
filesystem metadata. Normal operation is lower because compact summary/detail
records do not fill their caps and escalation is absent. Recorder health
reports dropped records, late cycles, write/sync failures, rotations, recovery,
torn/corrupt records, source timeouts, and truncation.

## Debug bundles and classification

The server allowlists only manifests, early-boot snapshots, eight current and
eight previous segments, and bounded current/previous pstore entries. Every
artifact has an explicit bundle read cap and symlinks/unrecognized files are
excluded. `host_flight_recorder_summary.json` reports sample ranges, cadence
gaps, valid/torn/corrupt counts, source-status totals, final observed state, and
escalation triggers.

Classification uses persisted planned intent first, then pstore, then a
non-zero watchdog bootstatus. Otherwise it reports
`unplanned_restart_unknown`. Observed undervoltage/throttling can support a
brownout hypothesis but never proves instantaneous external power removal;
external power telemetry remains authoritative.
