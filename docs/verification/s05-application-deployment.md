# S05 Application Deployment: verification

Server: the disposable Lightsail Ubuntu 24.04 server provisioned in S04 (`<deploy-server-ip>`, 2 GB). Driven by the user through the Deploy app. Results read from the app's local SQLite record (read-only) and the user's screenshots.

## 7.2 Static app (first run)

Created on 2026-11-28 through the Deploy UI.

- Type Static, slug `static-test`, domain `static.ossiframe.test`, repository `https://github.com/mdn/beginner-html-site-styled.git`, branch `main`, entry file `index.html`.
- The "Disable the NGINX default site" option was on for this app, so the plan used `site-apply-disable-default`. The NGINX default site is now disabled on this server.
- Operation result: Succeeded. All 16 steps succeeded, in order: `base-dirs`, `user-add`, `app-user-verify`, `app-root-check`, `app-root-owner`, `app-root-mode`, `releases-dir`, `shared-dir`, `private-dirs`, `site-apply-disable-default`, `release-clone`, `release-commit`, `boot-check`, `release-switch-link`, `release-switch`, `release-cleanup`.
- Release `20261128T113654Z`, commit `6c7a360`, branch `main`. App state Active.
- Verification, both essential checks passed:
  - `current` points to release `20261128T113654Z`.
  - HTTP 200 from the server with the `X-Ossiframe-App` header for this app (local request with the app's Host header).

Not checked in this run: opening the site from a browser (the `.test` domain does not resolve publicly), file modes and ownership on the server, and that the NGINX default site file in `sites-available` is untouched.

## Preflight after the app was created

The S04 preflight on the same server, run after the static app was created, showed class "Server provisioned by Ossiframe" and no listener blockers. That confirms the `d937118` fix on a real server: Ossiframe's own NGINX and MySQL listeners no longer block it.

It showed one blocker, "Installed software could not be listed: package list is incomplete". Cause, from the code: the shared `linux-inventory` parser treated a `dpkg-query` line with an empty version as unrecognized, even when the package is not installed (`un` or `pn` entries have no version), and any unrecognized line marks the list incomplete. Fixed so that non-installed lines are ignored and only installed lines need a version. Not yet re-checked on the server.

Warnings at the same time: apt or dpkg running, and a reboot pending.
