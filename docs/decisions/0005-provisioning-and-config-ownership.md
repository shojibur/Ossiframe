# ADR-0005: Server provisioning and configuration ownership

Status: Accepted (2026-11-24). S04 notes below are design facts; live behaviour is pending verification (S04 task 8).

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

## S04 resolutions
- **Composer:** the Ubuntu archive package (universe). The official installer is rejected because it downloads and runs code from the network, against the archive-only rule.
- **Waiting for `unattended-upgrades`:** preflight warns on apt/dpkg activity. The install step uses `-o DPkg::Lock::Timeout=300`. Ossiframe never kills apt or dpkg. Pending verification on a fresh image.
- **Package sources:** preflight blocks any enabled source outside the allowlist (`archive.ubuntu.com`, `*.archive.ubuntu.com`, `security.ubuntu.com`, `ports.ubuntu.com`), and the candidate gate re-checks origins after `apt-get update`. No PPAs. Sources are never modified.
- **Existing servers:** classified `ExistingUnmanaged` and blocked; no adoption. `OssiframeManaged` requires a local Succeeded operation for the same identity, never package presence.
- **Config files:** S04 writes none. The default distro configuration is used, and the config-write primitive (back up, write, validate, reload, restore) is deferred to S05.

## Open questions
- MySQL root access: S04 verification uses `sudo -n /usr/bin/mysql --no-defaults` over `auth_socket` (pending verification). Per-app database users are S05.
- Provider mirror hosts for the source allowlist: added only after verification.
