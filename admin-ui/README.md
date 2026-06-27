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
