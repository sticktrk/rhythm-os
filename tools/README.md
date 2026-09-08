# Tools

Developer, backend and packaging tooling. Production credentials and internal schedules are not included.

- `tools/app/scripts/` - Flutter app build, codegen, mobile, Android, and web helpers.
- `tools/app/supabase/` - Supabase config, edge functions, and database migrations.
- `tools/app/rhythm_harness.py` - App harness utility.
- `tools/os/scripts/` - Rhythm OS build, release, deployment, and rpiz image helpers.
- `tools/deploy-supabase.sh` - Apply pending database migrations and then deploy
  every canonical Edge Function.
- `tools/deploy-supabase-functions.sh` - Explicitly deploy canonical Supabase
  Edge Functions to the linked hosted project.
- `tools/config/` - Credential-profile schemas, examples, privacy-safe
  provisioning, loading, and validation for local and scheduled tooling. Live
  values stay outside the repository under `~/.config/rhythm/`.
- `tools/check-repo-invariants.sh` - Source boundaries, licenses and deployment fixture checks.
- `tools/install-git-hooks.sh` - Install the local source-check hook.
- `tools/ci/detect-changed-surfaces.sh` - Shared path classifier for CI jobs.

Deploy a product-only Supabase backend from the repository root:

```bash
./tools/deploy-supabase.sh
./tools/deploy-supabase.sh --dry-run
```

For the existing shared product/marketing database, compose both histories:

```bash
./tools/deploy-supabase.sh --marketing-repo rhythm-marketing --dry-run
./tools/deploy-supabase.sh --marketing-repo rhythm-marketing
```

Deploy only selected Supabase Edge Functions when needed:

```bash
./tools/deploy-supabase-functions.sh report-bug
./tools/deploy-supabase-functions.sh delete-user
./tools/deploy-supabase-functions.sh --all
```

See the [Supabase deployment guide](../docs/supabase.md) for first-time linking,
CI authentication, secrets, verification, direct CLI equivalents, and the
reason `--prune` must not be used.

Install the local source-check hook once per clone:

```bash
./tools/install-git-hooks.sh
```
