# Rhythm Admin API

Staff-only HTTP API for the web admin dashboard.

## Run

```sh
cd admin-api
dart pub get
python3 ../tools/config/run.py --consumer admin-api -- dart run bin/server.dart
```

The wrapper validates and loads the external `admin-api` profile (default
`~/.config/rhythm/admin-api.env`) without copying it into the repository. The
server also honors process-environment overrides. Provision or validate the
profile from the repository root with:

```sh
python3 tools/config/provision.py --profile admin-api
python3 tools/config/validate.py --consumer admin-api
```

Required values:

- `SUPABASE_URL`
- `SUPABASE_ANON_KEY`

Optional:

- `SUPABASE_SERVICE_ROLE_KEY`: lets the API read raw hub rows server-side for
  legacy plaintext tokens. Client-side encrypted hub tokens remain unavailable
  to the API.
- `SUPPORT_ACCESS_ENCRYPTION_KEY`: enables customer-granted beta admin support
  access. Must match the Supabase `support-access-grant` function secret.
- `SUPPORT_ACCESS_KEY_ID`: optional key id for support access envelopes;
  defaults to `default`.
- `ADMIN_API_PORT`: defaults to `8787`
- `ADMIN_API_ALLOWED_ORIGINS`: comma-separated browser origins allowed by CORS

## Support access key

`SUPPORT_ACCESS_ENCRYPTION_KEY` is the shared server-side secret used for
customer-granted beta admin support access.

Generate a new key with:

```sh
openssl rand -base64 32 | tr '+/' '-_' | tr -d '='
```

Set the exact same value in both places:

```sh
supabase --workdir tools/app secrets set SUPPORT_ACCESS_ENCRYPTION_KEY='<generated-value>'
export SUPPORT_ACCESS_ENCRYPTION_KEY='<generated-value>'
```

For deployed admin-api environments, store the same value in the service's
secret manager or `.env`; never commit it. The Supabase
`support-access-grant` Edge Function uses this key to encrypt durable support
tokens before writing `hub_support_access_grants`. `admin-api` uses the same key
to decrypt those grants server-side, then mints short-lived device support
session tokens for admin actions such as probes.

If the key is missing or differs between Supabase and admin-api, affected
devices remain reachable but authenticated probes fall back to `auth_required`.
`GET /health` reports `supportAccessConfigured` so agents can quickly check
whether admin-api has a key loaded. `SUPPORT_ACCESS_KEY_ID` defaults to
`default`; if you change it for rotation, update both Supabase and admin-api at
the same time.

## Endpoints

- `GET /health`
- `GET /ready`
- `GET /api/me`
- `GET /api/support/snapshot`
- `GET /api/fleet/hubs`
- `DELETE /api/hubs/:hubId` (admin role only)
- `POST /api/hubs/:hubId/probe`
- `GET /api/hubs/:hubId/status`
- `POST /api/hubs/:hubId/ota/check`
- `POST /api/hubs/:hubId/ota/update`
- `POST /api/hubs/:hubId/debug-bundle`
- `GET /api/hubs/:hubId/logs`
- `GET /api/hubs/:hubId/logs/:sourceId/tail?lines=120`
- `POST /api/hubs/:hubId/fleet-report`
- `POST /api/hubs/:hubId/device-admin/proxy`

All `/api/*` routes require `Authorization: Bearer <Supabase access token>` and
an enabled row in `public.rhythm_staff`.

`DELETE /api/hubs/:hubId` additionally requires the `admin` staff role and
`SUPABASE_SERVICE_ROLE_KEY`. It deletes only the matching Rhythm `server` hub;
the Home remains. Hub-scoped rows with foreign-key cascades are removed, but no
command is sent to the physical Light Box. A customer app retaining that hub in
local state may sync it back later.

`GET /health` is liveness. `GET /ready` is operational readiness for remote rpiz
support; it returns HTTP 503 until `SUPABASE_SERVICE_ROLE_KEY` and
`SUPPORT_ACCESS_ENCRYPTION_KEY` are configured. Check it locally with:

```sh
curl -s http://127.0.0.1:8787/ready
```

`POST /api/hubs/:hubId/debug-bundle` asks the appliance to upload through a
temporary signed URL in the private support bucket, reads the completed object
back through the service-role connection, deletes the temporary object, and
returns a gzip attachment. Large archives therefore do not traverse the remote
tunnel response. This review-only route does not create a support submission or
GitHub issue; hub tokens, signed URLs, and service-role credentials stay inside
`admin-api`.

The log routes use the same remote-first endpoint and token selection as probes
and bundles. They let staff inspect available rpiz log files and tail a selected
source without exposing device credentials to the browser.

`GET /api/fleet/hubs` returns every enabled Rhythm server hub without requiring
a matching Home/customer row. `POST /api/hubs/:hubId/fleet-report` is intended
for findings that Codex has already confirmed from the review-only bundle. It
securely uploads a fresh representative bundle, records a staff-owned debug
submission, and invokes the existing `report-bug` function. Both routes are
staff-gated, and fleet reporting requires `/ready` to be healthy.

`GET /api/hubs/:hubId/status` returns a compact remote support snapshot from
`/health`, `/api/state`, `/api/remote-access/status`, `/api/auth/status`, and
`/api/ota/status`. Sections are independent, so auth or OTA failures are shown
in an `errors` map while successful sections still render.

The OTA routes use the same remote-first endpoint and server-side token
selection as probes. `ota/check` asks the device to check the stable feed, and
`ota/update` asks the device to apply the available update.

### Device admin mutation contract

Proxy `GET` requests and `POST api/curve` previews are read-only. Every other
proxy request requires the `admin` role plus `requestId` and
`expectedServerInstanceId`; the API reads live `/api/state` from the selected
endpoint and returns 409 instead of dispatching when identity differs.

`PUT api/config` also requires a `resourcePrecondition` targeting the identical
path/query and carrying the canonical `bodySha256` returned by a prior proxy
GET. The API re-reads and hashes that resource before dispatch, and updated
appliances enforce the identity and hash again atomically with the config write,
preventing a reviewed proposal from overwriting a newer customer or staff edit.
It forwards the request ID as `X-Request-Id` and returns identity/hash
correlation fields in the receipt.

Deploy the appliance support first, then the admin UI, and finally the admin
API. The new UI collects these guards automatically and fails closed if an
older API does not return the config hash. An older UI against the hardened API
receives HTTP 428 for unguarded writes. The admin API retains its preflight for
older appliances, but atomic overwrite protection requires the updated
appliance.
