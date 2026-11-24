# ADR-0005: Server provisioning and configuration ownership

Status: Accepted (2026-11-24), with open questions.

## Decision
- Fresh-server provisioning targets Ubuntu 24.04 first. Ubuntu 26.04 is added only after verification.
- Before changes, a read-only preflight checks connectivity, OS and version, existing packages and services, existing web and database setups, package-manager locks, disk and memory. It produces a plan the user approves.
- Provisioning refuses servers that already look configured. Existing servers can still be inspected (Doctor). Adopting existing setups is a later capability.
- Software is installed through independent modules (NGINX, PHP-FPM, Composer, MySQL, Git). A server is never locked to its first profile.
- Packages come from the Ubuntu archive. Third-party repositories (for example for multiple PHP versions) need a recorded decision first.
- Config files Ossiframe manages carry an `ossiframe-` prefix. Unmanaged files are never overwritten.
- Config changes: back up, write, validate (`nginx -t` or equivalent), reload, verify; restore the backup if validation or verification fails.
- Initial database: MySQL, bound to localhost. PostgreSQL later.
- The server's observed state is authoritative; local records are cache.

## Open questions (resolve in S04)
- Composer from the Ubuntu archive or the official installer with checksum verification.
- How to wait for `unattended-upgrades` to release the dpkg lock on fresh VPS images.
- MySQL root access on Ubuntu (socket authentication via sudo) and creating per-app users.
