# Security

Ossiframe apps hold credentials that are often root-equivalent. Controls below are design and implementation facts unless marked verified in `docs/verification/`. Live behaviour of provisioning (S04) is pending verification.

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

### Privilege model (S04)

- Only passwordless sudo is supported. There is no sudo password, and no stdin path.
- Privileged commands are `Privilege::Root` specs: an absolute program path, rendered as `'/usr/bin/sudo' '-n' '--' '<program>' ...`. `sudo -n` fails instead of prompting. Every word is individually quoted. The one privileged command before approval is the preflight probe `/usr/bin/sudo -n -- /usr/bin/true`. A non-zero exit is never read as passwordless.
- Passwordless sudo is root-equivalent. Deploy's UI says so.

What enforces this, in layers:

1. **Registered modules.** An app can run only the modules it registers. `ubuntu-provisioning` registers zero `run_operation` operations.
2. **Typed IPC.** The ten `provision_*` commands take ids, component ids and a digest. They take no program, argv, path or privilege. They exist only when an app enables the `provisioning` feature, calls `provisioning::setup`, and lists them in its handler, `build.rs` manifest and capability file. Doctor does none of these; drift tests assert it.
3. **Provisioning is opt-in.** Deploy enables it. Doctor does not. Generated apps get it only with `--enable-provisioning`.
4. **`ReadOnly` guard on `run_operation`.** Modules get an executor that rejects `Privilege::Root` with `PrivilegeDenied` before anything is sent.
5. **Approval binding.** Privileged work runs only from an `ApprovedPlan`, built only by the plan store after the approval checks. The backend runs its own stored plan, not content from the frontend.

Limits, stated plainly:

- Modules are trusted compiled code and are not sandboxed. Trusted Rust code can build its own executor and call `sudo`. The `ReadOnly` guard checks the `Privilege` field only. It rejects only `Privilege::Root`. It does not classify commands and does not stop a user-level command from changing server state.
- The plan digest binds consent to what the user saw and detects tampering with a stored plan. It does not protect against a compromised frontend or a compromised desktop.
- The remote lock is cooperative. It excludes other Ossiframe instances only. It does not stop a person or another tool from running apt. A reboot clears it.
- APT installs transitive dependencies and runs maintainer scripts as root. The package source allowlist (Ubuntu archive hosts) and the candidate gate limit where packages come from. They do not limit what the packages' scripts do. The plan lists this before approval.
- Ossiframe does not roll back, does not run `dpkg --configure -a`, does not remove packages and never terminates apt, dpkg or a step unit.
- Operation history stores bounded, best-effort redacted output and no secrets.

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
| Concurrent changes | Per-server remote lock unit (cooperative, Ossiframe-only); one active operation per server locally |
| Unknown outcome retried | `UnknownOutcome` state; no automatic retry; read-only reconcile; user-initiated mark-resolved gated on fresh evidence |
| SSH or firewall lockout | Confirm a second working session before committing; never remove the last working access path |
| Public database exposure | Bind to localhost by default; Doctor flags public bindings |
| Supply chain | Committed lockfiles, `cargo audit`, pinned CI actions |
| Leaky reports | Redact secrets and env values in exports |

Out of scope: malware already running as the user on the desktop; kernel-level isolation between hosted apps.

## Reporting

A `SECURITY.md` with a private reporting channel will be added before the first release.
