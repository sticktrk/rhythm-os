# Rhythm Admin UI

React web dashboard for Rhythm staff.

## Run

```sh
cd admin-ui
cp .env.example .env
npm install
npm run dev
```

The UI signs staff in with Supabase Auth and sends the access token to
`admin-api`. The browser never receives hub tokens or service-role credentials.
When `VITE_SUPABASE_URL` or `VITE_SUPABASE_ANON_KEY` are not set in
`admin-ui/.env`, Vite falls back to the existing Flutter app `.env` values for
those two browser-safe settings.

From the dashboard, staff can probe a Light Box to verify remote/local
reachability, open a detailed status summary, inspect remote rpiz log tails, and
collect its `rhythm-debug-bundle-*.tar.gz` attachment through `admin-api` for
deeper debugging.

The dashboard reads `GET /ready` from `admin-api` after sign-in and warns when
remote rpiz debugging is not fully configured. A ready deployment reports
`remoteDebugReady: true`.
