# Implementation Plan: Ossiframe Doctor (S03)

## Overview

Rust (Tokio, Tauri 2) and TypeScript (React). Order: maintenance corrections, `linux-inventory` discovery, `linux-audit` crate, report builder, Doctor backend, Doctor frontend, generator catalog and docs, one consolidated quality gate, live verification, evidence. Tests are small inline unit tests written with the code they cover. No property tests, no e2e suites, no commit or push tasks. Do not implement anything beyond `design.md`.

## Tasks

- [x] 1. Maintenance corrections
  - [x] 1.1 Write `OSSIFRAME-LICENSE` in generated projects
    - In `tools/ossiframe-cli`, embed the repository `LICENSE` with `include_str!` and write `OSSIFRAME-LICENSE` at the generated project root when copied Ossiframe source is present. Build the directory list from the generation plan and name all of it: `packages/ui`, `packages/client`, every copied module UI directory (for example `src/modules/linux-inventory`, `src/modules/ports`) and every copied module crate (for example `modules/ports`). Do not touch the developer's own license.
    - Mention the file in the `templates/starter` README.
    - Unit test: the file is written, contains the MIT text, and names the shared packages and a copied module UI directory and crate.
    - _Requirements: 1.1, 1.2, 1.3, 1.4, 16.8_
  - [x] 1.2 Add the `--rev` compatibility note to `docs/developer-guide.md`
    - State that an explicit `--rev` must be compatible with the current generator, frontend packages and starter template.
    - _Requirements: 1.5_
  - [x] 1.3 Sanitize tracked public docs
    - Search `docs/` and other tracked public docs. Replace server IPs with `<server-ip>`, SSH key paths with `<key-path>`, personal home paths with `<home>/...`. Keep observations and results unchanged; add no evidence; do not rewrite git history. Fix other documentation inconsistencies found in edited files and list each fix in the task notes.
    - _Requirements: 1.6, 1.7, 1.9, 1.10_
  - [x] 1.4 Correct the `docs/roadmap.md` keychain risk row
    - Say that unsigned-macOS keychain behavior is not yet verified (the S01 checklist item is unchecked).
    - _Requirements: 1.8_

- [x] 2. Extend `linux-inventory` discovery
  - [x] 2.1 Extend `server-info`
    - Add `tokio` (`time`, `macros`) to `modules/linux-inventory`. Add `swapTotalKb`, `swapFreeKb`, `filesystems` (≤ 50, `filesystemsTruncated` and a limitation on truncation) from `df -P -B1` with `-x` for tmpfs, devtmpfs, squashfs, overlay, efivarfs, proc, sysfs, cgroup2, devpts and ramfs, dropping zero-size and snap mounts in the parser, and `cpuUsagePercent` from two `/proc/stat` samples with a cancel-aware 1 s sleep. Derive `rootFs` from `filesystems`. Return `null` CPU on a missing or unparsable sample or zero delta. A timed-out command gives an error without raw output.
    - Inline unit tests: `df`, `/proc/stat` delta (zero delta, missing sample), swap parsing, malformed input.
    - _Requirements: 2.1, 2.2, 2.3, 2.4, 2.5, 2.6, 16.1_
  - [x] 2.2 Add the `services` operation
    - Run `systemctl list-units --all` and `list-unit-files` with `--output=json` and merge by unit name. Return unit, description, installed, load, active, sub, enablement, with `unknown` where systemd reports nothing. Cap at 500 with `truncated` and a limitation. Unparsable output gives `unavailable`.
    - Inline unit tests: valid JSON, malformed JSON, truncation, installed-but-not-loaded unit gets `unknown` states.
    - _Requirements: 3.1, 3.2, 3.3, 3.4, 3.5, 16.1_
  - [x] 2.3 Add the `listening-sockets` operation
    - Run `ss -H -tulnp`. Return protocol, address, port, family, Binding_Class, and process or `null`. Strip zone suffixes before classifying. Cap at 300 with `truncated` and a limitation.
    - Inline unit tests: IPv4, IPv6, `%lo`, `*`, with and without process, malformed lines.
    - _Requirements: 4.1, 4.2, 4.3, 4.5, 16.1_
  - [x] 2.4 Add the `software` operation
    - Run `dpkg-query -W` with the fixed package list from the design. Return only status `ii`. Accept exit 1 with partial output. Run no version commands.
    - Inline unit tests: partial results, non-`ii` status, empty output.
    - _Requirements: 5.1, 5.2, 5.3, 16.1_
  - [x] 2.5 Update `@ossiframe/client` types
    - Add the new `ServerInfo` fields to `packages/client` types (`number | null` where a value can be missing) and types for the three new operation results.
    - _Requirements: 13.8_

- [x] 3. Create the `linux-audit` crate
  - [x] 3.1 Crate skeleton, model and step dispatch
    - Create `modules/linux-audit` (`ossiframe-mod-linux-audit`) with manifest dependency on `linux-inventory`. Add `model.rs` (`CheckResult`, `StepResult`, `Outcome` including `incomplete`, `Severity`, `Category`, summary counts, Coverage and Scan_Status, no score), the `OPERATIONS` list in scan order, `null`-only argument rejection, evidence limits (10 lines, 200 chars, control characters stripped, 100 checks per step), and shared probe-error mapping (permission or missing file → `unavailable`/`notApplicable`; bad exit, unparsable or truncated → `error`; timeout → `error` and continue; cancel and connection loss abort). Use only predefined `CommandSpec`, no `sudo`.
    - Unit tests with a fake executor: argument rejection, permission denied, cancel and connection-loss propagation, evidence limits.
    - _Requirements: 6.1, 6.2, 6.3, 6.4, 6.5, 6.6, 6.7, 6.9, 7.1, 7.2, 7.3, 7.4, 7.5, 11.1, 11.2, 11.4, 11.5, 16.5_
  - [x] 3.2 `resources`, `services`, `network` steps and rules
    - In `rules.rs` (approved thresholds as constants): `mem.low-available` (snapshot limitation text), `disk.low-space` (per meaningful filesystem, highest severity of 80/90/95% and < 100 MiB), `svc.failed` (≤ 20 findings; passed only on a complete list; truncated list gives `incomplete`), `net.database-exposed` (loopback none, private/link-local low, wildcard medium, other specific medium; `inferred` unless process visible; "non-loopback address" wording; truncated sockets give `incomplete`). No CPU rule. Each step collects evidence once using inventory parsers.
    - Inline rule tests: threshold boundaries, highest severity per filesystem, missing data gives `unavailable`, truncation gives `incomplete`, database severity by binding class, `inferred`, wording.
    - _Requirements: 6.8, 7.6, 8.1, 8.2, 8.3, 8.4, 8.5, 8.6, 8.7, 8.8, 8.9, 16.3_
  - [x] 3.3 `sshd.rs` and the `ssh` step
    - Read `sshd_config` with `Include` expansion (relative to `/etc/ssh`, lexical order, depth ≤ 4, ≤ 32 files, pattern validation, fixed-argv `find`). Limited reader, not a full OpenSSH parser. Case-insensitive keywords, `Keyword=value`, quotes stripped, first value wins, stop at `Match` with `matchBlocks: true` and "not fully assessed" (would-be pass becomes `incomplete`). Unreadable include → `unavailable`. Unsupported pattern or depth/file limit → `incomplete`, never pass. Unknown value → `error`. Rules `ssh.password-auth` and `ssh.root-login`: `prohibit-password`, `without-password`, absent and `forced-commands-only` are `passed` with a limitation and restriction text, never "root login disabled"; `no` passed; `yes` high. Evidence is the two keyword lines and file names. State that results describe global config on disk, not per-user settings or the running daemon.
    - Inline tests: first-value-wins across includes, `=` form, case, Match stop, unreadable include, depth and file limits, skipped include not a pass, unknown value, root-login wording, hostile patterns.
    - _Requirements: 9.1, 9.2, 9.3, 9.4, 9.5, 9.6, 9.7, 9.8, 9.9, 9.10, 9.11, 16.2_
  - [x] 3.4 `firewall` and `updates` steps
    - Rules `fw.ufw-config`, `upd.security-pending`, `upd.metadata-stale`, `upd.reboot-required` from `ufw.conf`, `updates-available`, `stat` stamps and `date -u +%s`. Never run `apt update`, `apt list --upgradable` or `apt-check`. Label update results as cached data with its age; zero pending with stale or unknown metadata is `incomplete`, never "fully patched".
    - Inline tests: `updates-available` parsing, stale boundary at 7 days, zero pending with stale cache gives `incomplete`, missing files give `unavailable`, `ENABLED` values.
    - _Requirements: 10.1, 10.2, 10.3, 10.4, 10.5, 10.6, 10.7, 10.8, 16.1, 16.3_
  - [x] 3.5 `certs.rs` and the `certificates` step
    - Discover nginx config with fixed-argv `find` and `cat` (≤ 32 files, 256 KB each). Extract `ssl_certificate` paths, validate (absolute, ≤ 255, charset, no `..`, four allowed prefixes, ≤ 20), skip others with a limitation. Run `openssl x509 -enddate -noout -in <path>`. Rule `tls.cert-expiry` (expired high, < 14 d high, < 30 d medium, no nginx `notApplicable`, unreadable `unavailable`, limits hit or paths skipped with no finding `incomplete`). Cover only certificates in readable nginx config, never claim a certificate is served, never read key files or return whole configs.
    - Inline tests: path validator (traversal, variables, quotes, newlines, leading `-`, wrong prefix), `openssl -enddate` parsing, expiry boundaries.
    - _Requirements: 10.9, 10.10, 10.11, 10.12, 10.13, 10.14, 16.1, 16.4_

- [x] 4. Report builder
  - [x] 4.1 Implement `report.rs`
    - `ReportInput`, `build_json` (schemaVersion 1, fields per design including scan status, coverage, timestamps and limitations; no key path or username; `host` omitted when redacted), `build_html` (self-contained, single `esc()`, no scripts, no remote assets, CSP meta tag). Both are deterministic apart from `generatedAt`. No secrets or config content beyond evidence lines.
    - Inline tests: `<script>` escaped in hostname and evidence, host redaction removes only `host`, secret-like string in server fields not emitted, JSON fields present, two builds differ only in `generatedAt`.
    - _Requirements: 14.1, 14.2, 14.3, 14.4, 14.10, 16.6_

- [x] 5. Doctor backend
  - [x] 5.1 Register modules and add the report commands
    - In `apps/doctor/src-tauri`, register `linux-inventory` and `linux-audit` explicitly. Add `build_report` (typed request with `deny_unknown_fields`, ≤ 4 MiB, ≤ 8 steps, ≤ 100 checks per step, bounded strings, no I/O) and `save_report` (rebuilds from the same request, native save dialog from Rust via `tauri-plugin-dialog` that confirms overwrite, writes only the chosen path, `false` on cancel, short error on write failure, no path or content from the frontend). Spike the dialog first and fall back to `rfd` if the plugin needs frontend permissions. Update `build.rs`, `capabilities/main.json` and the invoke handler; no `dialog:` or `fs:` permission. Regenerate permission files.
    - Unit test: capability file and `build.rs` command lists match (shared commands plus the two report commands) and contain no `dialog:` or `fs:` entries.
    - _Requirements: 11.3, 14.6, 14.7, 14.8, 14.9, 16.7, 18.10_

- [x] 6. Doctor frontend
  - [x] 6.1 Scan store, audit types and severity badge
    - Add `apps/doctor/src/audit-types.ts` and a module-level `scan-store.ts`: sequential steps in the order from `list_modules`, progress "step n of N", cancel via `cancelOperation`, partial results kept, labels "Incomplete: cancelled before <step>", timeout recorded and scan continues, connection loss marks results stale with age, no automatic retry, cleared on server removal, memory only.
    - Lifecycle: per-server `scanId` incremented on each scan, responses with an old `scanId` ignored, previous results cleared at scan start, results keyed by server id and `host:port` endpoint, host or port edit invalidates results, scan status and coverage kept separate.
    - Add a severity badge to `@ossiframe/ui`.
    - _Requirements: 12.1, 12.2, 12.3, 12.4, 12.5, 12.7, 12.8, 12.9, 12.10, 12.11, 12.12, 12.13, 12.14, 13.7_
  - [x] 6.2 Findings view, Diagnostics and Security pages
    - `FindingsView` with severity and outcome filters, text search, and a detail panel (evidence, explanation, recommendation, limitations, observed time, `inferred` badge). Diagnostics shows all results. Security reuses it filtered to `security`, `network`, `updates`. Loading, `unavailable` with reason and failure states. Remote strings rendered as text.
    - _Requirements: 13.3, 13.4, 13.5, 13.6, 11.6_
  - [x] 6.3 Overview, Services, Network, Software pages and navigation
    - Tabs: Overview, Diagnostics, Services, Network, Software, Security, Reports. Overview has summary cards (severity counts, coverage, scan status), scan controls, progress, completeness label, and extended inventory (swap, filesystems, CPU). Use existing shared UI only; no new dependencies, animations or charts. Services shows installed, loaded, active, failed, enabled and `unknown` separately. Services, Network (process "not visible" when `null`) and Software (states that non-dpkg software is not detected) read from step evidence without re-collecting. Searchable tables with loading, unavailable and failure states.
    - _Requirements: 4.4, 5.4, 12.6, 13.1, 13.2, 13.6_
  - [x] 6.4 Reports page
    - Freeze one assessment snapshot and redaction value, preview from `build_report`, send the same object to `save_report` for Export JSON and Export HTML. Tell the user to review the report before sharing. Label the toggle "Omit connection address" and state that hostnames, service names and paths remain. Short error on save failure with nothing else changed.
    - _Requirements: 14.3, 14.5, 14.7, 14.11, 14.12_

- [x] 7. Generator catalog and docs
  - [x] 7.1 Add the `linux-audit` catalog entry
    - In `tools/ossiframe-cli/src/plan.rs` add `linux-audit` (crate `ossiframe-mod-linux-audit`, type `ossiframe_mod_linux_audit::LinuxAudit`, dir `modules/linux-audit`, no UI dir) and the CLI crate dependency needed to read its manifest. No Doctor UI in generated apps.
    - Unit test: `linux-audit` resolves and adds `linux-inventory`.
    - _Requirements: 15.1, 15.2, 15.3, 16.8_
  - [x] 7.2 Update developer guide and architecture docs
    - List `linux-inventory` and `linux-audit` in the module catalog, update the `--modules` text, rename the auditing box in `docs/architecture.md` to `linux-audit`.
    - _Requirements: 15.4, 15.5_

- [x] 8. Consolidated quality gate
  - Run `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo check`, `cargo test --workspace`, `pnpm typecheck`, `pnpm lint`, production builds (including Doctor `pnpm tauri build`), `pnpm audit`, `cargo audit`, and `shellcheck` for touched scripts. Fix failures. Record any tool that could not run.
  - _Requirements: 16.9, 18.12_

- [x] 9. Live verification (disposable Lightsail Ubuntu 24.04 server only; read-only; never induce, fabricate or fix findings; never modify the server; record unverifiable checks as not verified). Tasks 1 to 8 do not wait for 9.1; only live verification is blocked by the fingerprint confirmation.
  - [x] 9.1 Confirm the host fingerprint
    - Stop and ask the user to confirm the host fingerprint via the Lightsail browser console (ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub); do not run any live check without it.
    - _Requirements: 17.1, 17.2_
  - [x] 9.2 Read-only discovery comparison
    - After confirmation, run only predefined read-only commands over OpenSSH pinned to the confirmed key (memory, `df`, `ss`, `dpkg-query`, `sshd_config` and `sshd_config.d`, `updates-available`, failed units). Compare with the Doctor module output. Note an `ss` socket without a visible process and root-only `unavailable` data. If the server has no findings, record that.
    - _Requirements: 17.1, 17.3, 17.4, 17.5, 18.4_
  - [x] 9.3 Manual UI checks by the user
    - Ask the user to run Doctor against the server and check: scan progress, cancel mid-scan with partial results and the incomplete label, disconnect mid-scan, `unavailable` outcomes, all seven pages, JSON and HTML export, report review for escaping and the redaction toggle, and a `grep` of exported files for password, passphrase and key material. Record the user's reported results only.
    - _Requirements: 17.6, 17.7, 18.8, 18.9, 18.10_
  - [x] 9.4 Generated-app reuse check
    - Generate an app with `--modules linux-audit`, confirm `linux-inventory` is added, build it, and run one `runOperation` call against the server through the public `@ossiframe/client` interface.
    - _Requirements: 17.8, 18.11_

- [x] 10. Record evidence and update docs
  - [x] 10.1 Write `docs/verification/s03-ossiframe-doctor.md`
    - Sanitized evidence (`<server-ip>`, `<key-path>`, `<home>/...`). Map each of the 13 S03 completion criteria to its evidence. List anything not run as not verified. Never fabricate output.
    - _Requirements: 17.10, 18.13_
  - [x] 10.2 Update `docs/roadmap.md` and `docs/compatibility.md`
    - Mark S03 done in the roadmap with open checks listed. Add to the compatibility doc only facts observed in verification.
    - _Requirements: 17.10, 18.13_

## Notes

- All test work is inline in the implementing task. There are no optional tasks, no separate test checkpoints and no commit or push tasks.
- Tasks 9.2 to 9.4 touch only the disposable server the user names, and only with read-only commands.
- The owner's 13 S03 completion criteria are Requirement 18.

## Task Dependency Graph

```json
{
  "waves": [
    { "id": 0, "tasks": ["1.1", "1.2", "1.3", "2.1"] },
    { "id": 1, "tasks": ["1.4", "2.2"] },
    { "id": 2, "tasks": ["2.3"] },
    { "id": 3, "tasks": ["2.4"] },
    { "id": 4, "tasks": ["2.5", "3.1"] },
    { "id": 5, "tasks": ["3.2"] },
    { "id": 6, "tasks": ["3.3"] },
    { "id": 7, "tasks": ["3.4"] },
    { "id": 8, "tasks": ["3.5"] },
    { "id": 9, "tasks": ["4.1"] },
    { "id": 10, "tasks": ["5.1"] },
    { "id": 11, "tasks": ["6.1", "7.1"] },
    { "id": 12, "tasks": ["6.2", "7.2"] },
    { "id": 13, "tasks": ["6.3"] },
    { "id": 14, "tasks": ["6.4"] },
    { "id": 15, "tasks": ["8"] },
    { "id": 16, "tasks": ["9.1"] },
    { "id": 17, "tasks": ["9.2"] },
    { "id": 18, "tasks": ["9.3"] },
    { "id": 19, "tasks": ["9.4"] },
    { "id": 20, "tasks": ["10.1"] },
    { "id": 21, "tasks": ["10.2"] }
  ]
}
```
