# ADR-0006: Application and runtime extensibility

Status: Accepted (2026-11-24)

## Decision
- A server is a set of observed facts and installed capabilities, never a runtime-typed server. Runtime families and versions can coexist.
- Laravel is the first application framework, implemented as a module outside Core.
- Each application gets a dedicated Linux user, its own directory, its own `.env` (mode 0600), logs and, for PHP, its own PHP-FPM pool and socket. NGINX is the shared public entry point.
- Proposed on-server layout: `/srv/ossiframe/apps/<slug>/`, `ossiframe-<slug>.conf` for NGINX vhosts and FPM pools. Finalized in S05.
- First deployment strategy: in-place Git update with a per-app lock, a user-editable deploy script, streamed output and a health check. Migrations need separate approval. Atomic releases come later.
- Future runtimes (Node.js, Python, containers) run as services under the app user on an internal port behind NGINX. Python apps use their own virtual environment and never modify the system Python.
- No runtime or framework adapter abstraction until a second implementation exists. Laravel and PHP are built concretely first.

## Consequences
- Per-user separation stops one app from reading another's files when permissions are correct. It is not container or VM isolation, and docs must not describe it that way.
- Rolling back code does not reverse migrations or data changes.
