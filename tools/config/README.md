# Local credential profiles

Rhythm's checked-in tooling owns the credential schema, examples, validation,
and loading behavior in this directory. Live credentials do **not** belong in
the repository or in an isolated worktree. Their default location is:

```text
~/.config/rhythm/
```

The five profiles are intentionally separated by capability:

| Profile | Default file | Intended consumer |
|---|---|---|
| `staff` | `staff.env` | Read-only bundle, fleet, and support tools |
| `analytics` | `analytics.env` | Aggregate PostHog reporting |
| `admin-api` | `admin-api.env` | Local Admin API service |
| `app-build` | `app-build.env` | Flutter compile-time configuration |
| `release` | `release.env` | Explicit local Cloudflare R2 upload |

Every live profile must be a regular, current-user-owned file with mode `0600`
or stricter. Symlinks, relative override paths, undeclared keys, duplicate
keys, malformed lines, and incomplete authentication alternatives fail closed.
Only non-empty process environment variables override file values.

Validate the checked-in contract without reading live credentials:

```bash
python3 tools/config/validate.py --schema-only
```

Provision external profiles from the existing ignored app, Admin API, and
release files without printing values or overwriting an existing profile:

```bash
python3 tools/config/provision.py --all --check
python3 tools/config/provision.py --all
```

Provisioning splits the mixed legacy files by capability, writes new files
with mode `0600`, and sets the external directory to `0700`. A complete durable
staff email/password pair is preferred over copying a potentially short-lived
access token. Incomplete profiles are not created.

Validate a provisioned profile or a consumer's complete profile set:

```bash
python3 tools/config/validate.py --profile staff
python3 tools/config/validate.py --consumer fleet-scout
```

Run any consumer through the same loader without copying credentials into a
worktree:

```bash
python3 tools/config/run.py --consumer fleet-scout -- python3 path/to/consumer.py
```

The wrapper validates the selected profiles and adds only their declared keys
to the child process environment. It does not invoke a shell or print values or
resolved paths. The child process remains responsible for keeping its own
output privacy-safe.

`RHYTHM_CONFIG_DIR` changes the shared configuration directory. Each profile
also has one explicit absolute-path override documented in `profiles.toml`.
Validation reports only profile state and key names; it never prints credential
values or resolved local paths.

## Migration status

The schema/loader foundation and external-profile provisioning are now in
place. Existing application, Admin API, release, fleet, and scheduled-task
consumers still use their legacy locations until they are migrated and tested
separately. Do not remove those files or assume this validator has changed
runtime behavior yet.
