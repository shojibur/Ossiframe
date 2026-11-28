# Known limitations

What Ossiframe does not do yet, and what has not been checked on a real server. Each item names its source. Items are removed only when they are fixed or verified, with the evidence recorded in `docs/verification/`.

## Platforms

- Desktop builds exist for macOS on Apple Silicon only. Intel macOS, Linux and Windows are not built or tested. (ADR-0007, S06 design)
- Release builds are ad-hoc signed and not notarized, so macOS asks for confirmation on first launch. (S06 design)
- There is no auto-updater. Updates are a manual download. (S06 design)
- Servers: Ubuntu 24.04 only. Ubuntu 26.04 is not verified. (`docs/compatibility.md`)

## Not verified on a real server

### SSH and credentials (S01)

Source: `docs/verification/s01-skeleton-foundation.md`.

- Encrypted private keys: passphrase prompt, wrong passphrase, saved passphrase.
- Password authentication. It is implemented but has never been tested; password login is disabled on the test server.
- Reconnect after an app restart with a saved secret.
- Detecting a network drop through keepalive.
- A changed host key blocking the connection, and the replacement flow.
- Errors for an unreachable host or a wrong port.
- Editing host or port deleting the saved secret.
- Keychain behavior with an unsigned or ad-hoc signed build.
- Idle memory of the running app.

### Doctor (S03)

Source: `docs/verification/s03-ossiframe-doctor.md`.

- A visible `unavailable` outcome in the UI was never observed live; it is covered by unit tests only.
- Certificate discovery, positive matching of `php*-fpm` packages, and server-side logs were not examined.

### Provisioning (S04)

Source: `docs/verification/s04-infrastructure-provisioning.md`.

- One full install succeeded on a fresh Ubuntu 24.04 server with every component verified, and a later preflight confirmed that a provisioned server no longer blocks itself on its own listeners (`d937118`). Not run yet:
  - the fix for the "package list is incomplete" preflight blocker;
  - a verify-only re-run on an already provisioned server;
  - rejection after a server change, after plan expiry, and after a changed digest;
  - a second plan or desktop seeing the server lock as busy;
  - edits and profile removal refused during an operation;
  - an SSH disconnect during `apt-get install`, then Reconcile and "Mark resolved";
  - the `DPkg::Lock::Timeout` wait while unattended-upgrades holds the dpkg lock;
  - Doctor against a provisioned server.
- The dpkg health check reads package states without root. It does not cover the missing-metadata checks of `dpkg --audit`.

### Application deployment (S05)

Source: `docs/verification/s05-application-deployment.md`. One static app was created and deployed on a real server with both health checks passing. Not verified yet:

- Opening a deployed site from a browser.
- PHP and Laravel app creation and deployment, and the database.
- Private repositories with a deploy key, and pinned Git host keys.
- The boot check, the release switch, rollback and removal.
- The HTTP health check and its `X-Ossiframe-App` header.
- That `install` from `/dev/stdin`, setgid directory modes and `open_basedir` behave as designed on Ubuntu 24.04.

## Not supported yet

### Provisioning (S04)

- Base stack only: NGINX, PHP 8.3 with PHP-FPM, Composer, MySQL, Git and optional Redis, from the Ubuntu archive. No PostgreSQL, no other PHP versions, no third-party package sources. (S04 requirements)
- Updates and upgrades of installed packages are shown as information only; there is no upgrade action. (S04 design)
- No firewall changes. (S04 approval dialog)
- Passwordless sudo is required. (S04 requirements)

### Application deployment (S05)

Source: S05 `tasks.md` notes and `design.md`.

- App types: Laravel, generic PHP and static HTML. WordPress, Node.js, Python and Docker are not supported.
- No TLS or Let's Encrypt; sites are served over HTTP only.
- The "Use a test address" button fills in `<slug>.<server-ip>.sslip.io`, which depends on the free third-party sslip.io DNS service. It appears only when the server's host is a public IPv4 address. Anyone who knows the address can open the site.
- No queue workers and no scheduler.
- No user-defined deploy steps.
- No database password rotation.
- PHP-FPM `pm.max_children` is fixed at 5 per app.
- Private repositories need SSH with a deploy key. Private HTTPS repositories are not supported.
- One operation at a time per server. Two apps on one server cannot deploy at the same time.
- Rollback switches code only. It does not undo database migrations.
- A failed health check after the switch leaves the new release live; you roll back yourself.
- A Laravel app with no route at `/` fails the HTTP check, because the check requires a 2xx or 3xx response.
- A plan that carries secrets (the database password or `.env` changes) is held in memory only. If the app restarts before you approve, create the plan again.
- A failed create keeps its slug reserved. Remove the app and create it again under a different slug.
- The Operation tab notice for an unknown outcome names the tab but is not a clickable link.

### Generator

- The `app-deploy` module is not offered by the project generator. Its Tauri commands live in the Deploy app, so a generated app cannot use it. (S06 design)
