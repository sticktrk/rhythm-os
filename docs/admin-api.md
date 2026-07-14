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
