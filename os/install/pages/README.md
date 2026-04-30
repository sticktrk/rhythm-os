# Cloudflare Pages: get.rhythm.lighting

Static assets served at `https://get.rhythm.lighting/`. The bootstrap installer
lives here as `install.sh` so users can run:    

```
curl -fsSL https://get.rhythm.lighting/install.sh | bash
```

## Layout

| File            | Served as                | Purpose                                        |
| --------------- | ------------------------ | ---------------------------------------------- |
| `install.sh`    | `/install.sh`, `/`       | Bootstrap installer (downloaded by curl).      |
| `latest.txt`    | `/latest.txt`            | Plain text tag (e.g. `v0.4.176-beta`). CI updates this on each tag push; the bootstrap reads it to resolve "latest". |
| `installed.txt` | `/installed.txt`         | 1-byte target for the post-install hit-counter ping. Cloudflare logs the request with its query string. |
| `_headers`      | (Cloudflare directive)   | Sets `Content-Type` and `Cache-Control`.       |
| `_redirects`    | (Cloudflare directive)   | Maps `/` to `/install.sh` (302).               |
| `index.html`    | (only if `_redirects` removed) | Minimal landing page with the curl command. |

## Cloudflare Pages setup

This is configured **outside** the repo — Cloudflare Pages is wired to the
GitHub repo via the dashboard. One-time setup:

1. **Create a Pages project** in the Cloudflare dashboard, connected to
   `sticktrk/rhythm-os`.
2. **Build settings:**
   - Framework preset: *None*.
   - Build command: *(leave empty)*.
   - Build output directory: `install/pages`.
   - Production branch: `master`.
3. **Custom domain:** add `get.rhythm.lighting` and have it CNAME to the
   `<project>.pages.dev` host Cloudflare provides. Cloudflare auto-issues a
   TLS cert.
4. **Verify:**
   ```
   curl -I https://get.rhythm.lighting/install.sh
   curl     https://get.rhythm.lighting/latest.txt
   ```
   The script should come back with `Content-Type: text/x-shellscript` and
   `latest.txt` should be a single line containing the latest tag.

## Updating `latest.txt`

The CI workflow (`.github/workflows/ci.yml`, job `update-latest-pointer`)
updates this file on every successful release tag and either commits to
`master` directly or opens an auto-merge PR. Don't edit it by hand unless
you're debugging.

## Reading install counts

The bootstrap pings `/installed.txt?p=<platform>&v=<version>&os=<uname>&m=<user|system>&e=<install|uninstall>`
on success (unless `RHYTHM_NO_TELEMETRY=1`). To read counts:

- **Cloudflare Web Analytics** on the Pages project shows page views per path
  and per query string.
- **Cloudflare Logpush / Logs** (paid plans) gives the full request log if you
  need fine-grained slicing by platform.

The `installed.txt` file itself contains a single byte; clients don't read it,
they only need the `200 OK` response so the request shows up in access logs.
