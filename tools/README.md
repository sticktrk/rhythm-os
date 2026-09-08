# Tools

These are the canonical reusable tools for public contributors and downstream
operations workspaces. Build/test/package/upload logic belongs here. Credentials
are supplied at runtime; company-specific approvals and publisher workflows can
remain in a separate operations repository.

Release commands accept `RHYTHM_RELEASE_PUBLISH_HOOK`, an absolute executable
path. After a successful tag push the hook receives the tag and its exact source
commit as two arguments. Dry runs, `--no-push` and failed pushes do not invoke it;
publisher failures make the release command fail. Without a hook, pushing a tag
does not upload an OTA release. `--upload` remains the explicit local upload path.

`RHYTHM_RELEASE_EVIDENCE_ROOT` selects the beta-verification receipt directory.
`RHYTHM_APP_BUILD_EVIDENCE_ROOT` selects the mobile build receipt directory;
`RHYTHM_APP_BUILD_RECEIPT_PATH` still overrides a single receipt's full path.
These paths let operations callers retain evidence outside the public checkout.

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
- `tools/ci/check-*.sh` and `check-commissioning-reuse.py` - product ownership guards run by public CI.
- `tools/ci/detect-changed-surfaces.sh` - Shared path classifier for CI jobs.

Deploy a product-only Supabase backend from the repository root:

```bash
./tools/deploy-supabase.sh
./tools/deploy-supabase.sh --dry-run
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
