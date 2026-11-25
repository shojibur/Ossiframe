# S03 Verification (Ossiframe Doctor)
Status: **verification recorded; S03 closes with open notes** (see "Completion criteria" and "Not verified"). Of the 13 criteria, 6 are Met and 7 are Met with note; none is wholly Pending, but the unverified parts are listed under "Not verified". Roadmap and compatibility updates (task 10.2) are separate.

## Environment (sanitized)
- Workstation: macOS, Apple Silicon (primary dev platform). Doctor and the generated app were unsigned local builds.
- Target: one disposable Lightsail Ubuntu 24.04.5 LTS server (`<server-ip>`), kernel 7.0.0-1014-aws, x86_64, systemd 255, hostname `ip-<private-ip>`, SSH user `ubuntu` (not root), key `<key-path>`. Read-only operations only; no sudo; the server was not modified.
- Doctor ran in its normal Tauri build. The generated app ran in Local_Mode (see task 9.4).
- Raw reference output and exported reports are kept outside the repository (`<home>/...`) and are not committed.

## Host fingerprint confirmation (closes the S01 identity item)
The ed25519 host key `SHA256:qobMUOP//FiL3rW2mD42syTV5iK5BzW0Dw9Rylibswk` was confirmed by the user in the Lightsail browser console (task 9.1) before it was pinned or trusted. `ssh-keyscan` output alone was not used as trust. This closes the S01 open item "independent confirmation of the server host key fingerprint" in `docs/verification/s01-skeleton-foundation.md`. Other S01 open checks stay open, in particular password authentication, which is unverified and remains disabled.

## Quality gates (task 8)
Run on macOS. The figures below are from a re-run at the time this file was written, with the same commands as task 8. The Doctor bundle build was not re-run for this file.

| Check | Result |
|---|---|
| `cargo fmt --all --check` | Pass |
| `cargo check --workspace --all-targets` | Pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | Pass, no warnings |
| `cargo test --workspace` | Pass: 240 passed, 0 failed, 1 ignored (the live SSH test in `ossiframe-ssh/tests/live.rs`, which needs a server and was not run). By crate: ossiframe-cli 36, ossiframe-core 28, ossiframe-desktop 32, ossiframe-doctor lib 7, ossiframe-mod-linux-audit 90, ossiframe-mod-linux-inventory 33, ossiframe-mod-ports-example 6, ossiframe-ssh 8. Doc-tests: 0 |
| `cargo test -p ossiframe-desktop --no-default-features` (no Tauri) | Pass: 29 passed, 0 failed |
| `pnpm -r typecheck` | Pass |
| `pnpm -r lint` | Pass |
| `pnpm audit` | No known vulnerabilities found |
| `cargo audit` | No vulnerabilities; 563 crate dependencies scanned, 1567 advisories loaded; 3 allowed warnings (below) |
| Doctor production bundle (`pnpm --filter @ossiframe/doctor tauri build`) | Pass at task 8, and again after the redaction default change (recorded under discrepancy 1). Not re-run when this file was written |
| `shellcheck` | Not run: no shell script was added or touched in S03 (none found in the repository outside `node_modules` and `target`), and `shellcheck` is not installed on this machine |

`cargo audit` warnings (same three as S01, none new):
- `chrono` 0.4.45, unmaintained, RUSTSEC-2026-0543, reached through `russh-util` (S01 record).
- `proc-macro-error` 1.0.4, unmaintained, RUSTSEC-2024-0370, through Tauri's Linux GTK stack.
- `glib` 0.18.5, unsound, RUSTSEC-2024-0429 (`VariantStrIter`; fixed in 0.20.0 or later), through Tauri's Linux GTK stack.
The two GTK warnings are not compiled into the macOS build (per S01 record). The dependency chains were taken from S01; they were not re-derived with `cargo tree` here. No fix is available for the first two.

## Reference evidence (read-only, OpenSSH)
- Date: 2026-11-25 (collected once, around 12:37 local time)
- Target: the disposable Lightsail Ubuntu 24.04 server (`<server-ip>`), user `ubuntu` (not root), key `<key-path>`.
- Method: one plain OpenSSH invocation with `IdentitiesOnly=yes`, `BatchMode=yes`, `StrictHostKeyChecking=yes`, a dedicated pinned known_hosts file and `HostKeyAlgorithms=ssh-ed25519`. No `ssh-keyscan`, no sudo, no apt, no changes. Read-only commands only.
- Host key: ed25519, `SHA256:qobMUOP//FiL3rW2mD42syTV5iK5BzW0Dw9Rylibswk`. The user confirmed this fingerprint independently in the Lightsail browser console (task 9.1) before the key was pinned. The connection succeeded under strict checking against that pinned key.
- Raw output: kept outside the repository and untracked (`<home>/ossiframe-s03-reference.txt`, mode 600). It is not committed.

### Observed values
| Item | Observed |
|---|---|
| Hostname | `ip-<private-ip>` (cloud default, private address form) |
| OS | Ubuntu 24.04.5 LTS (noble), `VERSION_ID="24.04"` |
| Kernel / arch | 7.0.0-1014-aws, x86_64 |
| systemd | 255 (255.4-1ubuntu8.18) |
| Memory | MemTotal 422132 kB, MemAvailable 194884 kB (about 46%), SwapTotal 0, SwapFree 0 |
| Disks (`df -P -B1`, filtered) | `/` 19682557952 B, 14% used, 16998342656 B available. `/boot` 923156480 B, 14%. `/boot/efi` 109395456 B, 6% |
| Service units (`list-units --all`) | 177 |
| Service unit files | 241 |
| Failed service units | none (empty output) |
| Listening sockets (`ss -H -tulnp`, as non-root) | udp: 127.0.0.54:53, 127.0.0.53%lo:53, 127.0.0.1:323, private-addr%ens5:68, [fe80::...]%ens5:546, [::1]:323. tcp: 127.0.0.53%lo:53, 0.0.0.0:22, 127.0.0.54:53, [::]:22. No `users:(...)` column on any line |
| Packages (dpkg-query) | installed (`ii`): `git` 1:2.43.0-1ubuntu7.4, `python3` 3.12.3-0ubuntu2.1. No package found for nginx, apache2, `php*-fpm`, `php*-cli`, composer, nodejs, mysql-server, mariadb-server, postgresql, redis-server, docker.io, docker-ce |
| `/etc/ssh/sshd_config` | `-rw-r--r-- root root`, 3571 bytes, world-readable |
| `/etc/ssh/sshd_config.d/` | one file: `60-cloudimg-settings.conf`, `-rw-r--r-- root root`, 26 bytes, world-readable |
| sshd directives found | `Include /etc/ssh/sshd_config.d/*.conf` (main file). `PasswordAuthentication no` (in `60-cloudimg-settings.conf`). No `PermitRootLogin` line. No `Match` line |
| ufw | `/etc/ufw/ufw.conf`: `ENABLED=no` |
| `updates-available` | readable. 38 updates can be applied immediately, 25 of them standard security updates. Also an ESM Apps notice |
| `update-success-stamp` mtime | 1795548690 |
| `reboot-required` | absent (`stat` reported no such file) |
| Server clock (`date -u +%s`) | 1795581425, so the stamp is 32735 s (about 9 h) old |
| `/etc/nginx` | absent |
| `openssl` | `/usr/bin/openssl` |

Not collected (not in the approved command list): certificate files, nftables or iptables rules, effective sshd settings (`sshd -T` needs root), per-user SSH settings.

### Implementation notes from this reference
- `systemctl list-units --type=service --all --no-pager --output=json` works on systemd 255 and returns a JSON array of objects with `unit`, `load`, `active`, `sub`, `description`. Only the first 300 bytes were captured, so full-document validity was not checked here. Older systemd versions may lack `--output=json` for `list-units`, so a fallback or an explicit incomplete outcome is still needed.
- `dpkg-query -W` with a glob pattern (`php*-fpm`) is accepted as a pattern. A pattern with no match prints `dpkg-query: no packages found matching <pattern>` on stderr, and the command exits non-zero, while matching packages in the same call (`git`, `python3`) are still printed to stdout. Doctor must not treat a non-zero exit as total failure, and must separate stdout rows from stderr "no packages found" lines. A positive glob match (an installed PHP package) was not exercised because none is installed.
- Package rows carry a status column (`ii `, with a trailing space). A package that is removed but has config files left (`rc`) would also appear, so installed must mean the `ii` state.
- `/etc/ssh/sshd_config` and `/etc/ssh/sshd_config.d/*.conf` are readable by a non-root user on this image, so no `unavailable` outcome is expected for SSH here. Doctor should still handle unreadable files.
- Without root, `ss -p` returned no process information for any socket. All listeners here are root or system services, which is consistent with the kernel hiding other users' process data. Doctor must show sockets without a visible process as "process not visible", and must not guess an owner.
- `ss -H -tulnp` prints the `Netid State Recv-Q Send-Q Local:Port Peer:Port` columns. Local addresses carry interface suffixes (`127.0.0.53%lo`, `%ens5`) and IPv6 scope, so address parsing must tolerate `%iface`.
- `stat` on multiple paths exits non-zero if any path is missing, but still prints the existing ones. A missing `reboot-required` is the normal "no reboot needed" state, not an error.
- `/var/lib/update-notifier/updates-available` is readable as non-root and contains the cached counts. It is cached data, not live apt state.
- The server's kernel string (7.0.0-1014-aws) is unusual for 24.04 but is recorded as observed.

## Expected Doctor outcomes on this server (expectations only)
These are derived from the observed values above and the rule thresholds in `modules/linux-audit` (memory medium below 10% available, disk medium at 80%, stale metadata after 7 days). They are to be compared with the real Doctor report in task 9.3. They are not results.

| Rule | Expected outcome | Basis |
|---|---|---|
| `mem.low-available` | Pass | About 46% available, above the 10% threshold. No swap is not itself a finding under this rule |
| `disk.low-space` | **Corrected.** Originally written as "Pass"; that was wrong. `/` and `/boot` pass, but `/boot/efi` (about 105 MB, about 98 MiB available) is below the 100 MiB absolute minimum that the rule had at the time, so a finding was expected under that rule. After the rule change (absolute minimum only for filesystems of 1 GiB or more) the expected result is Pass for all three filesystems | `df` values above. Usage is at most 14%, but the small EFI partition was overlooked |
| `svc.failed` | Pass (no failed units) | Failed-unit listing was empty |
| `net.database-exposed` | Pass | No listener on 3306, 5432, 6379, 27017, 11211 or 9200. Only 22 (all addresses) and local DNS and time ports |
| Software inventory | Not a rule. Present: git, python3. Absent: all others listed. Expected in the report's package list, not in the checks | dpkg-query output |
| `ssh.password-auth` | Pass | `PasswordAuthentication no` from `60-cloudimg-settings.conf` through the `Include`. Global setting only, with a note that `Match` and per-user settings are not assessed |
| `ssh.root-login` | Pass, from the default | No `PermitRootLogin` line, so the OpenSSH default `prohibit-password` applies. Expect a "not set, default applies" note |
| `fw.ufw-config` | Finding or not-protected result for ufw disabled in configuration. Wording and severity to be read from the rule | `ENABLED=no`. Effective nftables or iptables rules and the Lightsail cloud firewall are not inspected, so Doctor must not claim the server is unprotected |
| `upd.security-pending` | Finding, Medium severity | 25 security updates reported in the cached file |
| `upd.metadata-stale` | Pass (not stale) | Stamp is about 9 h old, below 7 days |
| Reboot required | Not required | `reboot-required` absent |
| Certificates step | Unknown from this reference | No certificate collection was run. Compare against what Doctor reports |
| Unavailable outcomes | Expected for any root-only data (process names in `ss`, effective firewall). None expected for sshd files, `df`, `/proc/meminfo` | Non-root user `ubuntu` |

The server therefore is not expected to be finding-free. The expected findings are the pending security updates and the disabled ufw configuration. Any other finding in the real report needs to be explained rather than accepted.

## Not verified
- Standalone_Mode (a generated app pinned to a published Ossiframe `rev`) was not re-verified in S03, because the S03 changes are not yet pushed. Generated-app reuse was verified in Local_Mode only.
- Disconnect during a running scan, and a visible `unavailable` outcome in the UI, were not explicitly reported by the user.
- The redaction default change (box ticked by default) in the Reports page is not UI-verified.
- Password authentication (S01 open check) remains unverified and disabled.
- `shellcheck` did not run (no scripts touched; tool not installed).
- Doctor bundle build was not re-run when this file was written.
- Socket count, unrecognized lines and truncation flag are not in the export, so they are inferred from the passed check.
- Host omission in the post-fix export (the address was present, see open discrepancy 1).
- Positive `php*-fpm` glob matching.
- Full JSON validity of the systemd listing (only the first 300 bytes were captured).
- Certificate discovery on the server.

## Doctor live results (task 9.3)
Same disposable server and user as the reference. Doctor ran read-only through its typed operations. Results come from the user's reported UI checks and from the exported JSON/HTML report. The files are kept outside the repository and are not committed.

### UI checks (user-confirmed, first run, before the disk fix)
- Run assessment with "step n of 8" progress and summary cards.
- Cancel after one or two steps: "Incomplete: cancelled before ..." shown, completed steps kept.
- Full assessment completed.
- Diagnostics filters, search and detail panel.
- Security preset.
- Services, Network and Software tables filled.
- Disconnect: results marked stale.
- Not reported by the user: disconnect during a running scan, a visible `unavailable` outcome in the UI. Not verified.
### Defects found and fixed (post-fix confirmation)
1. `disk.low-space` raised a HIGH finding on `/boot/efi` (about 105 MB, 5% used, about 98 MiB available). The absolute 100 MiB minimum now applies only to filesystems of 1 GiB or more. The earlier expectation in this file was also wrong and is corrected above. **Confirmed fixed live:** in the export from a Doctor process started after the rebuild (rebuild 13:00:27 +08; export files written 13:04:10 and 13:04:16 +08), `/`, `/boot` and `/boot/efi` all passed, with no high finding.
2. The network check ended `incomplete` ("socket list is truncated") because `ss` prints a link-local IPv6 address as `[addr]%iface:port` (zone after the bracket), which the parser skipped. The parser now handles that form. **Confirmed fixed live:** `net.database-exposed` passed ("No known database port listens on a non-loopback address") and `databasePortSockets` is empty. Scan coverage is assessed 12, notApplicable 1, notAssessed 0.

Limit of this confirmation: the export does not carry the parsed socket count, `unrecognizedLines` or a truncation flag. "10 sockets, 0 unrecognized lines, not truncated" is therefore inferred from the check passing instead of `incomplete`, not read from the file. Not verified directly.

### Save location (resolved, not a defect)
`apps/doctor/src-tauri/src/report_commands.rs` writes exactly to the path returned by the native save dialog, and the default name is `ossiframe-doctor-YYYYMMDD.<ext>`. The exported files carry the default name and the default folder (Documents), so the dialog defaults were accepted. Recorded as observed behaviour.

### Comparison: reference (task 9.2) against the post-fix exported JSON report
Memory, `df` used bytes and the update age are single observations taken at different times, so small differences are timing.

| Item | Expected (reference) | Doctor (post-fix export) | Match |
|---|---|---|---|
| Identity | Ubuntu 24.04.5 LTS, kernel 7.0.0-1014-aws, cloud default hostname | Same OS, kernel string and hostname | Yes |
| Memory | 422132 kB total, 194884 kB available (46%) | 422132 kB total, 202920 kB available (48%), `mem.low-available` passed | Yes (timing) |
| Swap | 0 / 0 | 0 / 0 | Yes |
| Filesystems | `/`, `/boot`, `/boot/efi`, same sizes | Same three, same total sizes. Used bytes differ by a few KB (timing). Not truncated | Yes |
| Disk usage percent | `df` shows 14%, 14%, 6% | Doctor shows 13%, 13%, 5% (used / (used + available), as stated in the check limitation) | Different method, documented |
| `disk.low-space` | Pass for all three | All three passed (`/boot/efi` 5%, 98 MiB available) | **Yes (fixed)** |
| Service units | 177 listed units, 241 unit files, 0 failed | `unitCount` 278, `failedCount` 0, `svc.failed` passed | Failed count yes. 278 is not 177 or 241 (open, see below) |
| Listening sockets | 10 sockets, no process visible | `net.database-exposed` passed, no database-port sockets. Socket total not in the export | **Yes (fixed)**, count inferred only |
| Packages | git 1:2.43.0-1ubuntu7.4, python3 3.12.3-0ubuntu2.1 only | Same two packages and versions | Yes |
| `ssh.password-auth` | Pass, global only | Passed, evidence names the drop-in file, limitation says global only, not per-user or running daemon | Yes |
| `ssh.root-login` | Pass, default applies | Passed, "Root login allowed with public key only", "not set; using the OpenSSH default prohibit-password". Never says "disabled" | Yes |
| `fw.ufw-config` | Finding, low | Finding, low, `ENABLED=no`, effective firewall not known | Yes |
| `upd.security-pending` | Finding, medium, cached data | Finding, medium, 25 security updates, cached-data limitation | Yes |
| `upd.metadata-stale` | Pass, about 9 h | Passed, "9 hours" | Yes |
| Reboot required | Not required | `upd.reboot-required` passed, marker absent | Yes |
| Certificates | Not collected in reference | `tls.cert-expiry` notApplicable (no nginx configuration); reference shows no `/etc/nginx` | Yes |
| `schemaVersion` | 1 | 1 | Yes |
| Scan status and coverage | Separate | status `complete`, `stale` false, no missing steps. 13 results: passed 10, finding 2, notApplicable 1, incomplete 0. Coverage assessed 12, notApplicable 1, notAssessed 0 | Yes |
| Host omitted | No address when omitted | **`server.hostRedacted` false, `server.host` present, note "The connection address is included in this report."** The box was unticked at export (default) | **No. Explained by the unticked default, see below** |

### Open discrepancies
1. **Connection address present in the 13:04 export (explained, not a redaction failure).** `ReportsPage.tsx` initialised `useState(false)`, so after a relaunch or tab switch the "Omit connection address" box started unticked. Export is blocked unless the preview's frozen redaction equals the toggle, and the preview label states whether the address is included, so the 13:04 export with `hostRedacted` false reflects an unticked box. Redaction was exercised by the earlier export at 12:52:59Z (`hostRedacted` true, connection address absent from the JSON and HTML). The redaction code is unchanged since. **Default changed after the live run:** the default is now `useState(true)` (privacy-preserving). This change is not live-verified in the UI. It is covered only by `pnpm -r typecheck`, `pnpm -r lint` and a successful `pnpm --filter @ossiframe/doctor tauri build`. The 13:04 export files still contain the connection address and must not be shared.
2. **Disk percentage method.** Doctor uses used / (used + available), rounded down, so it shows 13% / 13% / 5% against `df` 14% / 14% / 6%. Documented in the check limitation. A difference in method, not a defect.
3. **`unitCount` definition.** 278 matches neither the 177 listed units nor the 241 unit files. It is consistent with a merge of both lists by unit name (between 241 and 418), but the intended definition is not confirmed against the module.

### Export checks (post-fix JSON and HTML)
- Only IPv4 text in either file is the connection address (discrepancy 1). No other address. The private address appears only inside the cloud default hostname `ip-<private-ip>`, so the report carries address information even with the address omitted. The note warns about hostnames; the user should review before sharing.
- No `.pem` file name, no `PRIVATE KEY`, no `BEGIN ` marker, no home-directory paths in either file.
- HTML: no `<script>`, no remote URL, a Content-Security-Policy meta tag (`default-src 'none'`, inline styles only).
- Redaction verified by the earlier 12:52:59Z export: `hostRedacted` true and no connection address in the JSON or HTML. That file was overwritten by the 13:04 export (same file name), so the note wording could not be re-read from it. The wording is taken from `modules/linux-audit/src/report.rs` (`REDACTED_NOTE`): it says the connection address was omitted, that hostnames, service names, package versions, ports, process names and file paths are still present, and that the report "is not anonymized". It does not claim anonymization. A unit test in that file checks the "not anonymized" text. The 13:04 export (address included) carries the note "The connection address is included in this report."
- "Root login allowed with public key only" is accurate for the default and is not presented as a disabled root login.

## Generated-app reuse (task 9.4)

Local_Mode build (`--ossiframe-local`), generated with `--modules linux-audit`. No commits, no server contact during this step.

| Check | Result |
| --- | --- |
| `linux-inventory` added automatically | Yes. Generated `src-tauri/Cargo.toml` has both module dependencies and `lib.rs` registers both. |
| `OSSIFRAME-LICENSE` | Lists `packages/ui`, `packages/client`, `src/modules/linux-inventory`. Nothing for `linux-audit`. |
| Doctor code copied | None. The only "Doctor" text is a comment in the copied `packages/client/src/types.ts` ("Doctor inventory"). |
| App-defined page | `src/pages/AuditCheck.tsx`, registered in `src/modules.ts`. Calls `api.runOperation(serverId, "linux-audit", "resources" \| "ssh" \| "updates", null)` and shows ruleId, outcome, severity, title as text. Errors shown through `errorMessage`. |
| `pnpm install`, `pnpm typecheck`, `pnpm lint` | Pass |
| `pnpm tauri build` (full bundle) | Pass. Produced `.app` (16.8 MiB) and `.dmg` (5.6 MiB), unsigned. |
| `cargo test` in `src-tauri` | 2 passed, 0 failed |

Bundle: `<home>/ossiframe-s03-gen/audit-app/src-tauri/target/release/bundle/macos/Ossi Audit.app`

### Live `runOperation` call (user-reported)
The user ran the generated "Ossi Audit" app (Local_Mode, `--modules linux-audit`, app-defined `AuditCheck` page) against the Lightsail server. The app connected after the user trusted the fingerprint. The user reported "Worked": the `resources`, `ssh` and `updates` steps returned check tables through `runOperation`. This is the user's report only; no output, counts or screenshots were supplied, and none are recorded here. Standalone_Mode with a published rev was not tested (see "Not verified").

## Defects found during live verification
| Defect | Fix | Verification |
|---|---|---|
| `disk.low-space` raised a HIGH finding on the 105 MB `/boot/efi` partition (absolute 100 MiB minimum) | Absolute minimum now applies only to filesystems of 1 GiB or more | Confirmed live in a post-fix export |
| `ss` output `[addr]%zone:port` (zone after the bracket) was not parsed, so the network check ended `incomplete` | Socket parser handles that form | Confirmed live: `net.database-exposed` passed (socket count inferred, not in the export) |
| Report redaction box started unticked, so an export carried the connection address | Default changed to ticked (`useState(true)`) | Not UI-verified; typecheck, lint and bundle build only |

## Completion criteria (Requirement 18)
| # | Criterion | Status | Evidence and note |
|---|---|---|---|
| 1 | Doctor is an independently useful diagnostics app | Met | Seven pages, scan, findings, export, run live against a real server (task 9.3) |
| 2 | Connects to actual authorized Linux infrastructure | Met | Disposable Lightsail server, fingerprint confirmed in the console (9.1), read-only run (9.2, 9.3) |
| 3 | Existing connection management remains functional | Met with note | Connect after trust, and disconnect (results marked stale) worked in Doctor; connect worked in the generated app. Password auth is unverified and disabled |
| 4 | Reusable modules provide actual discovery data | Met | Memory, swap, filesystems, services, sockets and packages matched the OpenSSH reference (comparison table) |
| 5 | Checks give meaningful, evidence-based results | Met with note | Expected outcomes matched the real report. Open: df percentage method differs from `df`; `unitCount` 278 definition unconfirmed |
| 6 | Findings show severity and verification limits | Met | Severity and limitation text present in the export (ufw, updates, ssh) |
| 7 | Services, network and software available in the app | Met with note | Tables filled (user-confirmed). Socket count not in the export; `unitCount` unconfirmed |
| 8 | Progress, cancellation and partial results | Met with note | Progress, cancel with "Incomplete" label and kept partial results confirmed. Disconnect during a scan and a visible `unavailable` outcome not reported |
| 9 | JSON export works | Met | JSON and HTML exported, schema version 1, no scripts, no key material, redaction exercised (earlier export) |
| 10 | No unauthorized server mutations | Met with note | Only predefined read-only commands, no sudo, no mutating IPC; server unchanged as far as reported. Server-side logs were not examined |
| 11 | Modules reusable outside Doctor | Met with note | Generated app built and ran 3 steps via `runOperation` in Local_Mode. Standalone_Mode with a published rev not re-verified |
| 12 | Production builds and quality checks pass | Met with note | See quality gates. `shellcheck` not run (nothing to check); 3 `cargo audit` warnings; bundle not re-run today |
| 13 | Real-server verification recorded honestly | Met | This file: user-reported items marked as such, unverified items listed |
