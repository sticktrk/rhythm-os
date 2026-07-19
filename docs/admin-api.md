# Admin API

Launch the local staff admin API from the repo root with:

```sh
cd admin-api && dart pub get && dart run bin/server.dart
```

The API listens on `http://127.0.0.1:8787` by default.

## Environment

Create `admin-api/.env` before launching if it does not already exist:

```sh
test -f admin-api/.env || cp admin-api/.env.example admin-api/.env
```

Required values:

- `SUPABASE_URL`
- `SUPABASE_ANON_KEY`

Useful optional values:

- `SUPABASE_SERVICE_ROLE_KEY`
- `SUPPORT_ACCESS_ENCRYPTION_KEY`
- `SUPPORT_ACCESS_KEY_ID`
- `ADMIN_API_HOST`
- `ADMIN_API_PORT`
- `ADMIN_API_ALLOWED_ORIGINS`

The server loads env values from the process, `admin-api/.env`, and the Flutter
app env file when present.

## Check It

After the server starts, verify it locally with:

```sh
curl -s http://127.0.0.1:8787/health
curl -s http://127.0.0.1:8787/ready
```

`/health` is the liveness check. `/ready` returns HTTP 503 until remote support
dependencies such as `SUPABASE_SERVICE_ROLE_KEY` and
`SUPPORT_ACCESS_ENCRYPTION_KEY` are configured.

## Destructive actions

`DELETE /api/hubs/:hubId` is restricted to enabled staff with the `admin` role
and requires `SUPABASE_SERVICE_ROLE_KEY`. It removes only the matching Rhythm
`server` hub plus its hub-scoped cascade records. It does not delete the Home or
send a reset command to the physical Light Box. A customer app that still has
the hub locally may recreate the cloud record during a later sync.

## Guarded device mutations

`POST /api/hubs/:hubId/device-admin/proxy` keeps `GET` and device-sampled
`POST /api/curve` requests read-only. Every other proxied method is a live
customer-device mutation and therefore requires:

- an enabled staff account with the `admin` role;
- a safe 8-128 character `requestId`;
- `expectedServerInstanceId`, verified from `/api/state` on the same endpoint
  immediately before dispatch; and
- for `PUT /api/config`, a `resourcePrecondition` with the same path/query and
  canonical SHA-256 body hash from a prior proxy GET.

The API returns HTTP 428 for missing guards and HTTP 409 when identity or config
freshness no longer matches. Successful mutation responses include the request
ID, verified server identity, response hash, and precondition hash. The request
ID is forwarded as `X-Request-Id` so the appliance support audit can be matched
to the proposal/apply receipt.

Use the `rhythm-customer-lighting-tuning` skill for remote curve work. It
defaults to proposal-only, stores ignored before/candidate/after evidence and a
human-readable handoff, and applies only after separate explicit authorization.

Deploy the updated admin UI before the hardened admin API. The updated UI fails
closed on config writes when an older API does not return the required body
hash; older admin UIs receive explicit precondition errors from the hardened
API rather than issuing an unguarded mutation.
