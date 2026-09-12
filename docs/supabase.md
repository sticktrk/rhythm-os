# Supabase development and deployment

Rhythm OS includes the product backend in `tools/app/supabase/`: accounts, homes,
devices, support, remote access and device activity. Keep its configuration,
RLS policies, migrations and Edge Functions in the public source repository.

## Local product setup

Create local settings from `tools/app/supabase/.env.example`, then run:

```bash
supabase --workdir tools/app start
supabase --workdir tools/app db reset
supabase --workdir tools/app functions serve --env-file tools/app/supabase/.env
```

Use the local URL and anon/publishable key printed by the CLI in your app
configuration. Never put a service-role key in Flutter, the SDK or a browser.
`db reset` above is local; never add `--linked` to it against production.

## Deployment settings

Copy only the optional integration settings you actually use into the ignored
`tools/app/supabase/.env`. Keep it mode `0600`. Supabase supplies its own URL,
anon key and service-role key inside hosted functions; do not duplicate them.

- Remote access requires `RHYTHM_REMOTE_ACCESS_DOMAIN` and the Cloudflare
  account, zone and scoped API token. The domain is honored exactly; there is
  no upstream domain fallback or legacy-domain rewrite.
- GitHub support routing requires both `GITHUB_ISSUES_REPO` and
  `GITHUB_ISSUES_TOKEN`. Use a private repository and a token with repository
  metadata read and issue write access. Visibility is verified before sending
  support content. Missing configuration or failed verification leaves the
  submission stored with a routing error so it can retry after correction.
  Redirects are refused, and existing issue URLs must match the configured
  repository and issue number. After a repository rename, issue transfer, or
  routing change, correct the routing and stored association before retrying;
  an issue number by itself is not portable between repositories.
- Support encryption uses the existing `SUPPORT_ACCESS_ENCRYPTION_KEY`, shared
  with the Admin API. Preserve it across upgrades.
- Monster uses **three credentials per deployment**, `MONSTER_EMAIL`,
  `MONSTER_PASSWORD` and `MONSTER_AYLA_APP_SECRET`, for one dedicated vendor account. All authenticated app
  users share that broker by default. `MONSTER_OWNER_USER_ID` optionally
  restricts it to one user. There are no per-customer or per-device Edge
  Function secrets, and the short-lived ticket key is derived from the account
  email and password. The public vendor app ID stays in code; the app credential must be configured. Use
  `python3 tools/monster-cloud-secrets.py --project-ref <project-ref>` to
  provision credentials without retaining them in this checkout.

Secrets are shared by the Supabase project, not separately configured for
100 functions. Optional integrations only require their own configuration.
Existing hosted credentials do not need to be recreated when source moves.

After reviewing your selected project and local values, upload settings:

```bash
supabase --workdir tools/app secrets set --env-file tools/app/supabase/.env
```

This updates the named settings; it does not unset unrelated hosted secrets.
Deploying source does not automatically upload the local `.env`.

Subscription tiers were never launched. App entitlement enforcement, tier
queries, realtime subscriptions, demo overrides and plan UI are suppressed.
The retired `set-subscription-tier` endpoint returns HTTP 410 without database
access after deployment. Historical subscription tables/migrations remain for
released-client and database-history compatibility. An old
`ENTITLEMENTS_ENABLED=true` build setting cannot enable unfinished billing.

## Link your product project

```bash
supabase login
supabase --workdir tools/app link --project-ref <project-ref>
```

Generated `.temp/` metadata and `.env` are local and must never be tracked.
Use `SUPABASE_ACCESS_TOKEN` in CI. `SUPABASE_PROJECT_REF`, when supplied, must
match the linked project for combined database/function deployment.

## Deploy a product-only project

```bash
./tools/deploy-supabase.sh --dry-run
./tools/deploy-supabase.sh
```

The wrapper applies pending product migrations before deploying product
functions. Failure stops later stages. It never prunes hosted functions.
Deploy selected product functions without changing schema:

```bash
./tools/deploy-supabase-functions.sh report-bug --dry-run
./tools/deploy-supabase-functions.sh report-bug
./tools/deploy-supabase-functions.sh --all
```

Function authentication settings come from `config.toml`. Functions with
`verify_jwt = false` enforce their own user/token authorization where needed;
the retired subscription endpoint only returns an unavailable response.

## Deployments that share a database

A database with migrations owned by multiple repositories must receive the
complete migration history. An operations workspace should select the expected
project and exact source revisions before running the shared-history helper:

```bash
./tools/deploy-supabase.sh --marketing-repo /path/to/companion-repository --dry-run
```

This optional interface expects the companion history at `supabase/migrations`
and its deployment entrypoint at `scripts/deploy-supabase-functions.sh`. It
preserves SQL bytes and rejects duplicate versions and mismatched project links.
Do not deploy a partial history or repair applied migration records to fit one
checkout. A standalone product project uses only the product commands above.

## Verification and rollout

Review the pending migrations through the appropriate wrapper's `--dry-run`.
After explicit deployment, list functions and exercise the relevant flows:

```bash
supabase --workdir tools/app functions list
```

Deploy required configuration first, migrations second, functions third and
app/appliance artifacts last. Keep schema changes additive for released clients.
The source/configuration tests do not prove a live deployment or full RLS audit.
