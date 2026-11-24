# Compatibility

States: **Planned**, **Implemented** (code targets it), **Verified** (ran it; evidence linked), **Unsupported**. Only Verified entries are advertised as supported.

| Item | Version / arch | State | Evidence or note |
|---|---|---|---|
| Desktop: macOS | Apple Silicon (OS version not recorded) | Verified (limited) | Doctor and generated apps (Local_Mode and Standalone_Mode builds) built and ran there. See `docs/verification/s02-modules-and-generator.md`. |
| Desktop: macOS | Intel | Planned | |
| Desktop: Windows | 11 x64 | Planned | Needs WebView2 |
| Desktop: Linux | Ubuntu 24.04 x64 | Planned | Needs WebKitGTK and a Secret Service keyring |
| Server: Ubuntu | 24.04.5 LTS x86_64 | Verified (read-only) | Connection, inventory and listening TCP ports on one Lightsail instance. Nothing mutating. See `docs/verification/s02-modules-and-generator.md`. |
| Server: Ubuntu | 26.04 LTS | Planned | Only after verification |
| Server: OpenSSH | Ubuntu 24.04 archive version | Planned | Record actual version in S01 |
| NGINX, PHP-FPM, Composer, MySQL | Ubuntu 24.04 archive versions | Planned | Record actual versions in S04 |
| PostgreSQL | | Planned (later) | |
| Laravel | Current major at S05 time | Planned | |
