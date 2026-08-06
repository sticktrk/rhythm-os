# Cloudflare Pages: get.rhythm.lighting

Static assets served at `https://get.rhythm.lighting/`. The hosted desktop
binary installer was retired with the legacy `server/install/` CDN tree.
`install.sh` remains only as an explicit migration message for cached links;
the page directs appliance users to the canonical production SD-card image and
desktop users to build from source.

## Layout

| File            | Served as                | Purpose                                        |
| --------------- | ------------------------ | ---------------------------------------------- |
| `install.sh`    | `/install.sh`            | Fails with the desktop-installer retirement and migration instructions. |
| `_headers`      | (Cloudflare directive)   | Sets `Content-Type` and `Cache-Control`.       |
| `_redirects`    | (Cloudflare directive)   | Maps `/` to `/index.html` (302).               |
| `index.html`    | `/index.html`, `/`        | Source-build instructions and appliance image link. |

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
   curl -I https://dl.rhythm.lighting/server/sdcard.img.gz
   ```
   The retirement script should come back as `text/x-shellscript`; the factory
   image should be the latest stable/prod Buildroot disk image.
