# Rhythm Admin UI

React web dashboard for Rhythm staff.

## Run

```sh
cp admin-ui/.env.example admin-ui/.env
npm --prefix admin-ui install
python3 tools/config/run.py --consumer admin-ui -- npm --prefix admin-ui run dev
```

The UI signs staff in with Supabase Auth and sends the access token to
`admin-api`. The browser never receives hub tokens or service-role credentials.
The external `app-build` profile supplies the browser-safe Supabase URL and
anonymous key. Keep only UI-specific values such as `VITE_ADMIN_API_URL` in
`admin-ui/.env`; explicit `VITE_SUPABASE_URL` and `VITE_SUPABASE_ANON_KEY`
values there remain supported for deployment-specific builds. The UI never
reads the Flutter app's repository-local `.env`.

## Structure

Two surfaces behind one Supabase sign-in:

- **Home directory** (`/`, light theme) — a flat, searchable list of Homes
  with optional customer email, location, and timezone metadata. The list does
  not expose customer or hub records and does not contact appliances. Selecting
  a Home opens `/homes/:homeId`, which lists that Home's hubs without probing
  them.
- **Device console** (`/hubs/:hubId/*`, dark theme) — full-featured per-device
  admin opened from a Home's hub list or by direct URL. Side-nav sections:

| Route | Section |
|---|---|
| `overview` | Identity, vitals, day/sleep mode switch, light-breaker kill switch |
| `nodes` | Live room/node tree; per-node actions, brightness/CCT sliders, color wheel, preferences, time offset |
| `profiles` | Curve profile editor — device-sampled chart (`POST api/curve`), all five curve shapes, envelopes, timer unions, time simulator with absorb |
| `scenes` | Scene list/editor, apply, draft preview with commit/cancel |
| `modes` | Mode configs, transitions (clock dial trigger editor, manual trigger), light runtime |
| `inputs` | Input binding CRUD (action, target, raw JSON) |
| `topology` | Canonical devices, rooms, triage queue, pairing (Matter), hub credentials, Wi-Fi |
| `environment` | Location and solar times |
| `history` | Activity log with filters |
| `remote` | Cloudflare tunnel config/status, activity cloud |
| `security` | Auth status, owner claim, support tokens, require-auth toggle |
| `system` | Settings, OTA, logs, debug bundle, backup/restore, danger zone |
| `console` | Raw JSON request runner over the full operation catalog (escape hatch) |

All device traffic goes through `POST /api/hubs/:id/device-admin/proxy` on
admin-api — no new backend endpoints. Pages poll (5–60s, paused when the tab is
hidden) and refetch after every write; device payloads are parsed tolerantly
and every card has a "Raw JSON" disclosure as the escape hatch. Destructive
operations sit behind an in-app confirm dialog; the worst (factory reset,
backup restore, Wi-Fi reset) require typing a confirmation phrase.

`src/deviceAdminOperations.ts` remains the canonical operation catalog checked
by `scripts/check-device-admin-coverage.mjs` at build time — add new SDK
endpoints there first, then (optionally) to the typed layer in `src/device/`.

## Deployment note

The app uses `BrowserRouter`; whatever serves `dist/` must rewrite unknown
paths to `index.html` (Vite dev/preview do this automatically). If that is not
possible on your host, switch to `HashRouter` in `src/App.tsx`.

The dashboard reads `GET /ready` from `admin-api` after sign-in and warns when
remote rpiz debugging is not fully configured. A ready deployment reports
`remoteDebugReady: true`.
