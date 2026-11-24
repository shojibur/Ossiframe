# Security

Ossiframe apps hold credentials that are often root-equivalent. Controls below are planned unless marked verified in `docs/verification/`.

## Trust boundaries

| Boundary | Rule |
|---|---|
| WebView → Rust backend | Typed, intent-level IPC only. Backend validates every field. No command returns a secret. No generic shell or file IPC. Minimal Tauri capabilities and strict CSP. |
| Rust → keychain | Only `ossiframe-desktop`'s vault code reads or writes secrets. |
| Rust → SQLite | No secrets stored. |
| Desktop → server | SSH with a pinned host key. Remote output is untrusted input, rendered as text. |
| Management user → app users | Application code (deploy scripts, Composer, Artisan) never runs as root. |
| Server → internet | Packages from the Ubuntu archive only, until a decision adds another source. |

## Credentials

- Generated management keys are Ed25519 and stored in the OS keychain.
- Imported keys may be referenced by file path instead of copied.
- Bootstrap passwords stay in memory unless the user chooses to save them.
- Per-app database passwords are generated and written to the server's `.env` (mode 0600).
- Pinned host keys are public data and are stored in SQLite.
- Losing a keychain entry is recovered through the user's original credentials or provider console.

## Privilege

| Stage | Identity | Privilege |
|---|---|---|
| Bootstrap (Deploy) | Credentials the user provides | Root or sudo |
| Management (Deploy) | Dedicated management user (proposed: `ossiframe`) | Passwordless sudo via `sudo -n`. This is effectively root, and the docs say so. |
| Application | Per-app Linux user | No sudo |
| Diagnostics (Doctor) | User's chosen account | No sudo by default. Root-only reads are opt-in. |

Enforcement happens at the IPC and composition layers: each app exposes only the typed commands it needs, built from predefined command paths. Doctor exposes no mutating or privileged commands. Module metadata is not a sandbox, and trusted compiled code is not isolated from the app's own privileges.

## Main threats and controls

| Threat | Control |
|---|---|
| Compromised frontend runs commands | Typed IPC; plan then approve for mutations; backend executes only what it planned |
| SSH impersonation | Host key pinning; fingerprint shown before first trust; hard fail on change |
| Credential theft | Keychain only; redaction; no secrets in logs, IPC or SQLite |
| Command injection | Validated types; argv-based commands; one quoting function with focused tests |
| Path traversal | Paths built from validated names under fixed roots |
| Malicious server output | Bounded parsing; size caps; text rendering |
| Malicious repository content | Runs as the per-app user only |
| Broken config | Backup, validate (`nginx -t`), reload, verify, restore on failure |
| Concurrent changes | Per-server and per-app locks |
| Unknown outcome retried | `UnknownOutcome` state; manual inspection first |
| SSH or firewall lockout | Confirm a second working session before committing; never remove the last working access path |
| Public database exposure | Bind to localhost by default; Doctor flags public bindings |
| Supply chain | Committed lockfiles, `cargo audit`, pinned CI actions |
| Leaky reports | Redact secrets and env values in exports |

Out of scope: malware already running as the user on the desktop; kernel-level isolation between hosted apps.

## Reporting

A `SECURITY.md` with a private reporting channel will be added before the first release.
