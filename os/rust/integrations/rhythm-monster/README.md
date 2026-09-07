# rhythm-monster

A focused Monster/Ayla integration library for Rhythm static lighting. Supports
power, RGB (including Rhythm's Kelvin-to-RGB projection), and brightness. It does
not expose animations, music, addressable segments, native white channels or
native fades; transition requests apply the final state immediately.

The default model gate is `xt-16ft-hw-neon-led-rgbic` (Neon Flow). Other Monster
products need their own cloud-issued device key and a verified property/model
contract. Brand or account membership alone does not establish compatibility.

## Architecture

1. Discover an Ayla `ble::ID_SERVICE` candidate using the host's existing BLE
   scanner, then read its exact DSN with `LightBleTransport::identify`.
2. `LightCloudBroker::begin` authenticates the Rhythm caller and obtains a
   short-lived, owner/DSN-bound commissioning ticket. Monster email/password,
   partner-ticket exchange and Ayla login execute only in the edge function.
3. `LightCloudBroker::commission` writes the setup token and Wi-Fi credentials
   over app BLE first. `LightAppBleTransport` accepts a host-provided `LightGatt`
   bridge. If unavailable before writes, `LightBluezTransport` is the Linux
   fallback, behind the `bluez` Cargo feature and shared `rhythm-ble` admission.
   The host supplies a selected address from shared discovery; no private radio
   session, scanner or runtime is created here.
4. The broker registers the exact DSN using its setup token, verifies account
   ownership and supported writable properties, then returns its LAN key.
5. Construct one `LightLanClient` per device and share it through `Arc`. Its
   serialized lane sends authenticated LAN commands and requires signed matching
   readback. `LightMonsterController` implements Rhythm's controller traits.

This is the crate and broker delivery. Flutter GATT/FFI wiring, onboarding UI,
platform hub registration, capability advertisement and durable credential
storage are embedding work. Do not advertise the feature in an older app/server
until those surfaces are integrated. There are no existing store migrations.

Never automatically switch transports after an uncertain BLE write. Reconcile
cloud ownership / LAN reachability first. Keep the setup ticket privately until
completion or its ten-minute expiry; retry `credentials(dsn, Some(ticket))` if
cloud registration or LAN-IP propagation is still pending, without reprovisioning
Wi-Fi. For a device already on the account, use `credentials(dsn, None)`.

The app must close its GATT connection on success, error and cancellation, and
support a complete 105-byte GATT write-with-response. The BlueZ fallback runs a
bounded admitted operation and disconnects before completion. Cancelling an
outer caller does not cancel an already admitted server operation; reconcile its
outcome before retrying. The library owns no reset/unpair/persistence lifecycle.

## Appliance hub (Linux, `bluez` feature)

`hub::INTEGRATION` registers Monster as the `monster` hub type on the Linux
appliance and advertises the `monster_ble_nearby_scan` onboarding method. The
app drives three terminal `api/devices/pair` requests and brokers the cloud
steps between them, so the appliance never holds a Supabase session:

| Stage | Params | Result `details` |
| --- | --- | --- |
| `discover` | `scan_secs` (optional) | `candidates: [{dsn, address}]` from FE28 advertisers whose DSN was read over GATT |
| `provision` | `dsn`, `address`, `setup_token` | `dsn`; a failure sets `uncertain: true` after the first GATT write |
| `adopt` | `dsn`, `ip`, `local_key`, `local_key_id`, `name` | the paired light, only after a signed LAN power readback |

Wi-Fi credentials come from the appliance's stored commissioning credentials
(the same source Matter BLE commissioning uses). Adopted lights persist in
`<data_dir>/monster/devices.json` (0600 in a 0700 directory, corrupt files
quarantined) and are restored into one `LightLanClient` each on hub connect.
Unpairing removes the store entry and the canonical endpoint; it never calls
the cloud. Only the Bluetooth stages reserve the shared appliance adapter.

## Cloud setup

Deploy `tools/app/supabase/functions/monster-device` using the repository's normal
Supabase deployment workflow. Its entrypoint verifies the caller's Supabase JWT
through the existing shared auth middleware even though gateway `verify_jwt` is
false. By default the configured Monster account is shared: every authenticated
Rhythm user can commission lights and fetch LAN keys through it without creating
a vendor login of their own. Set `MONSTER_OWNER_USER_ID` to narrow the broker to
one Supabase user; all other callers are then refused before any vendor call.

Shared mode means any authenticated user who knows a DSN can fetch that device's
LAN key, since the vendor account cannot tell users apart. The key is only usable
from the device's own private LAN, and commissioning tickets still bind the user
who began setup, but per-user device claims are a follow-up before wide release.

Configure these secrets in the Supabase dashboard or from a private env file
outside the checkout using `supabase secrets set --env-file /private/path`:

| Secret | Value |
| --- | --- |
| `MONSTER_OWNER_USER_ID` | Optional. Leave unset to share the account (default); set a Supabase auth user UUID to restrict the broker to that one user |
| `MONSTER_EMAIL` | Monster account email |
| `MONSTER_PASSWORD` | Monster account password |
| `MONSTER_APP_ID` | Ayla app ID from the matching Monster application configuration |
| `MONSTER_APP_SECRET` | Matching Ayla application secret, kept server-side |
| `MONSTER_TICKET_SECRET` | Random signing secret of at least 32 characters |
| `MONSTER_MODEL_ALLOWLIST` | Optional comma-separated bench-verified model IDs |

Use `python3 tools/monster-cloud-secrets.py --project-ref YOUR_PROJECT_REF` to
enter these values without shell-history exposure and upload them through the
Supabase CLI. The helper generates a ticket-signing secret, uses a mode-0600
transient file, and removes it after upload. It does not deploy the function.
Pass `--ayla-config /private/app-config.json` to reuse the private Ayla
application configuration recovered during the earlier control test. That file
supplies `appId` and `appSecret`; you enter the account login and, optionally, a
Rhythm owner ID (press Enter to keep the account shared).
Re-running rotates the ticket secret and invalidates outstanding setup tickets.

The crate receives the HTTPS function URL and the authorized user's Supabase
access token. Never put a Supabase service-role key or Monster account password
on an appliance. Refresh the caller token through the host's existing Rhythm auth
flow. The broker caches provider access tokens in memory for at most five minutes;
there is no token database. Responses use `Cache-Control: no-store`; diagnostics
exclude provider bodies, account identifiers and secrets.

The proprietary cloud contract is pinned to the successfully tested Monster
Android `1.0.45(3)` login shape. Vendor changes may require a broker update.
Cloud credentials supplied in production secrets and a deployed live function
are still required; fixture tests do not establish deployment readiness.

## LAN bench

Store `{dsn, ip, local_key, local_key_id}` in a private mode-0600 file outside the
repository. Do not attach that file to diagnostics or backup without the host's
secret-storage policy. LAN credentials are per device; reacquire after a reset
or key rotation. `Debug` redacts credentials and secret values zeroize on drop.
The host must permit the device to reach the callback listener on its LAN
interface. Cloud access is not needed for ordinary lighting with a saved key.

```sh
cargo run -p rhythm-monster --example lan -- /private/device.json power
cargo run -p rhythm-monster --example lan -- /private/device.json off
cargo run -p rhythm-monster --example lan -- /private/device.json on
# Changes color/brightness briefly and restores saved static properties:
cargo run -p rhythm-monster --example lan -- /private/device.json static-test
```

## Validation / release gaps

Deterministic tests cover an independent AES/HMAC fixture, replay and tamper
rejection, callback queue continuation, exact DSN/Wi-Fi packet encoding,
post-write fallback refusal, owner/ticket isolation, registration idempotency,
capability checks, secret redaction and controller fan-out failures.

LAN power, static RGB and brightness with signed readback and original-state
restoration are bench verified on Neon Flow firmware 2.6.3 (2026-09-07). The
18-exchange static test exercises callback-port reuse and session deletion. Fresh-device BLE
commissioning, actual app GATT bridging, Linux adapter cleanup and mixed radio
workloads need a physical receipt before product enablement. Existing paired
hardware was not reset to perform this test. Trace fixtures use synthetic IDs;
no real LAN key, setup token, DSN, MAC or Wi-Fi details are committed.
