# Requirements Document

## Introduction

S03 turns the existing Doctor shell (`apps/doctor`) into a read-only diagnostics and auditing app for existing Linux servers. The work extends `linux-inventory` with discovery operations, adds a `linux-audit` module with evidence-based checks, adds a scan workflow and report export to Doctor, updates the generator catalog and docs, and applies four maintenance corrections. Requirements are derived from `design.md`. The final requirement lists the S03 completion criteria.

## Glossary

- **Doctor**: The `apps/doctor` desktop application.
- **Inventory_Module**: The `linux-inventory` module (`modules/linux-inventory`).
- **Audit_Module**: The new `linux-audit` module (`ossiframe-mod-linux-audit`, `modules/linux-audit`).
- **Executor**: The Core executor that runs a structured `CommandSpec` over SSH.
- **Step**: One Audit_Module operation (`resources`, `services`, `network`, `software`, `ssh`, `firewall`, `updates`, `certificates`), run with `null` arguments.
- **Scan**: A sequential run of all Steps for one connected server.
- **Check_Result**: One rule outcome with fields `ruleId`, `title`, `category`, `outcome`, `severity`, `resource`, `evidence`, `explanation`, `recommendation`, `observedAt`, `limitations`, `inferred`.
- **Outcome**: One of `passed`, `finding`, `unavailable`, `incomplete`, `notApplicable`, `error`. `incomplete` means evidence was partly collected (truncated, skipped or limited) so the check cannot pass; `unavailable` means no usable evidence.
- **Scan_Status**: Execution state of a Scan: `complete` (every Step ran to the end) or `incomplete` (cancelled, aborted or a Step failed).
- **Coverage**: Counts of Check_Results that were assessed (`passed`, `finding`), not assessed (`unavailable`, `incomplete`, `error`) and `notApplicable`. Coverage is independent of Scan_Status.
- **Scan_Id**: A per-server, monotonically increasing number that identifies one Scan.
- **Severity**: One of `critical`, `high`, `medium`, `low`, `info`.
- **Binding_Class**: Classification of a listening address as `loopback`, `wildcard` or `specific`.
- **Report_Builder**: The pure report functions in `linux-audit::report`.
- **Report_Commands**: The Doctor Tauri commands `build_report` and `save_report`.
- **Generator**: The `tools/ossiframe-cli` project generator.
- **Verification_Server**: The disposable Ubuntu 24.04 Lightsail server named by the user.
- **Quality_Gates**: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo check`, `cargo test --workspace`, `pnpm typecheck`, `pnpm lint`, production builds, `pnpm audit`, `cargo audit`, and `shellcheck` for touched scripts.

## Requirements

### Requirement 1: Maintenance corrections

**User Story:** As a maintainer, I want known documentation and licensing gaps fixed first, so that generated projects and public docs are correct before new work builds on them.

#### Acceptance Criteria

1. WHEN the Generator creates a project that contains copied Ossiframe source, THE Generator SHALL write an `OSSIFRAME-LICENSE` file at the project root containing the MIT text and copyright line from the repository `LICENSE`.
2. THE `OSSIFRAME-LICENSE` file SHALL name every copied Ossiframe directory: the shared packages (`packages/ui`, `packages/client`), every copied module UI directory (for example `src/modules/linux-inventory` and `src/modules/ports`) and every copied module crate (for example `modules/ports`). THE Generator SHALL derive this list from the generation plan and not from a fixed example.
3. THE Generator SHALL leave the license of the developer's own application unchanged.
4. THE starter template README SHALL mention the `OSSIFRAME-LICENSE` file.
5. THE `docs/developer-guide.md` SHALL state that an explicit `--rev` must be compatible with the current Generator, frontend packages and starter template.
6. THE tracked `docs/verification/*.md` files and any other tracked public document found by search SHALL contain `<server-ip>` in place of server IP addresses, `<key-path>` in place of SSH key paths, and `<home>/...` in place of personal home paths.
7. THE sanitized verification documents SHALL retain their observations and results.
8. THE `docs/roadmap.md` risk table SHALL state that unsigned-macOS keychain behavior is not yet verified.
9. THE sanitization SHALL NOT change recorded observations into new claims, SHALL NOT fabricate evidence, and SHALL NOT rewrite git history.
10. THE maintenance task SHALL also fix other documentation inconsistencies it finds in the files it edits, and SHALL list each fix in its task notes.

### Requirement 2: Extended server information

**User Story:** As a server operator, I want complete resource facts, so that I can see memory, swap, disk and CPU state of a server.

#### Acceptance Criteria

1. WHEN `server-info` runs, THE Inventory_Module SHALL return the existing fields plus `swapTotalKb`, `swapFreeKb`, `filesystems` and `cpuUsagePercent`.
2. THE Inventory_Module SHALL derive `rootFs` from `filesystems`.
3. WHEN `server-info` runs, THE Inventory_Module SHALL compute `cpuUsagePercent` from two `/proc/stat` samples taken 1 second apart, with a cancel-aware wait between them.
4. IF either `/proc/stat` sample is missing or unparsable, or the total delta is zero, THEN THE Inventory_Module SHALL return `null` for `cpuUsagePercent`.
5. THE Inventory_Module SHALL list at most 50 filesystems, SHALL set `filesystemsTruncated` and SHALL add a limitation string WHEN the list is truncated.
6. IF a `server-info` command times out after 10 seconds, THEN THE Inventory_Module SHALL return an error result for that command without returning raw output.
7. THE Inventory_Module SHALL list only meaningful filesystems: it SHALL exclude `tmpfs`, `devtmpfs`, `squashfs`, `overlay`, `efivarfs`, `proc`, `sysfs`, `cgroup2`, `devpts` and `ramfs` types, zero-size filesystems and mounts under `/snap/` or `/var/lib/snapd/snap/`.

### Requirement 3: Service discovery

**User Story:** As a server operator, I want to see service units and their state, so that I can find failed or disabled services.

#### Acceptance Criteria

1. WHEN the `services` operation runs, THE Inventory_Module SHALL return, for each service unit, the unit name, description, whether it is installed, load state, active state, sub state and enablement.
2. THE Inventory_Module SHALL obtain service data from `systemctl list-units --all` (loaded units) and `systemctl list-unit-files` (installed units) with `--output=json`, and SHALL merge them by unit name so that installed units that are not loaded are included.
3. IF systemd output is unparsable, THEN THE Inventory_Module SHALL return an `unavailable` result and not a guessed list.
4. THE Inventory_Module SHALL return at most 500 services, SHALL set `truncated` and SHALL add a limitation string WHEN the list is truncated.
5. WHERE a state is not reported by systemd for a unit (for example a unit file that is not loaded has no active state), THE Inventory_Module SHALL return `unknown` for that state and SHALL NOT infer it.
6. THE Doctor SHALL label the merged service count as "units (loaded ∪ installed)". THE Doctor Services page SHALL distinguish installed, loaded, active, inactive, failed, enabled, disabled and unknown, and SHALL NOT present inactive or disabled units as problems.

### Requirement 4: Listening socket discovery

**User Story:** As a server operator, I want to see listening sockets and how they are bound, so that I can see what is exposed on the host.

#### Acceptance Criteria

1. WHEN the `listening-sockets` operation runs, THE Inventory_Module SHALL return, for each socket, the protocol, local address, port, address family, Binding_Class, and process name and pid or `null`.
2. THE Inventory_Module SHALL classify `127.0.0.0/8` and `::1` as `loopback`, `0.0.0.0`, `::` and `*` as `wildcard`, and every other address as `specific`.
3. THE Inventory_Module SHALL strip zone suffixes such as `%lo` before classification.
4. WHERE `ss -p` returns no process for a socket, THE Inventory_Module SHALL return `process: null` and the Doctor UI SHALL display "not visible".
5. THE Inventory_Module SHALL return at most 300 sockets, SHALL set `truncated` and SHALL add a limitation string WHEN the list is truncated.

### Requirement 5: Software discovery

**User Story:** As a server operator, I want to see installed server software, so that I know which runtimes and services a host has.

#### Acceptance Criteria

1. WHEN the `software` operation runs, THE Inventory_Module SHALL query `dpkg-query` for the fixed package list in the design and return the name and version of each package with status `ii`.
2. IF `dpkg-query` exits with status 1 and partial output, THEN THE Inventory_Module SHALL return the packages found.
3. THE Inventory_Module SHALL NOT run version commands such as `nginx -v`.
4. THE Doctor Software page SHALL state that software installed outside dpkg is not detected.

### Requirement 6: Audit module structure and Check_Result model

**User Story:** As a module developer, I want audit results in one typed model, so that every consumer shows evidence the same way.

#### Acceptance Criteria

1. THE Audit_Module SHALL declare `linux-inventory` as a dependency in its manifest.
2. THE Audit_Module SHALL expose the Steps `resources`, `services`, `network`, `software`, `ssh`, `firewall`, `updates` and `certificates`, in that order, as its operation list.
3. IF a Step receives arguments other than `null`, THEN THE Audit_Module SHALL reject the call.
4. WHEN a Step runs, THE Audit_Module SHALL return the observation time, the evidence collected for that Step and a list of Check_Results.
5. THE Audit_Module SHALL give each Check_Result a stable `ruleId`, one Outcome, and a Severity only WHEN the Outcome is `finding`. WHEN a check cannot be completed because its evidence was truncated, skipped or limited, THE Audit_Module SHALL use Outcome `incomplete`.
6. THE Audit_Module SHALL limit each Check_Result to 10 evidence lines of at most 200 characters with control characters removed.
7. THE Audit_Module SHALL limit each Step to 100 Check_Results.
8. WHERE a conclusion goes beyond observed facts, THE Audit_Module SHALL set `inferred` to true.
9. THE Audit_Module SHALL report summary counts by Outcome and by Severity, plus Coverage, and SHALL NOT compute a security score.
10. THE Doctor and the reports SHALL show Scan_Status and Coverage separately. A Scan with Scan_Status `complete` and any `unavailable`, `incomplete` or `error` results SHALL NOT be described as a comprehensive or clean assessment.
11. THE Audit_Module SHALL keep valid findings found before an evidence limit was reached.

### Requirement 7: Step error handling

**User Story:** As a server operator, I want missing data reported honestly, so that I never read a missing check as a pass.

#### Acceptance Criteria

1. IF a probe fails with a permission error or a missing file, THEN THE Audit_Module SHALL return an `unavailable` Check_Result, or `notApplicable` WHEN the inspected subject does not exist on the server.
2. IF a probe returns an unexpected exit status, unparsable output or truncated output, THEN THE Audit_Module SHALL return an `error` Check_Result with a short message and without raw output.
3. IF one command times out, THEN THE Audit_Module SHALL return `error` for the dependent checks and continue with the rest of the Step.
4. IF a command is cancelled or the connection is lost, THEN THE Audit_Module SHALL abort the Step and return the error to the caller.
5. THE Audit_Module SHALL NOT return `passed` for a check whose data is missing, inaccessible or truncated.
6. WHEN evidence for a check was truncated by a limit and no violation was found in the evidence read, THE Audit_Module SHALL return `incomplete` and not `passed`.

### Requirement 8: Resource, service and network rules

**User Story:** As a server operator, I want threshold-based findings for resources, services and exposed ports, so that I can see problems with evidence.

#### Acceptance Criteria

1. WHEN `MemAvailable` is below 10% of total memory, THE Audit_Module SHALL return `mem.low-available` as a `medium` finding, and as a `high` finding WHEN below 5%. THE result SHALL state that it is a single snapshot and not evidence of sustained memory exhaustion.
2. WHEN a filesystem is at least 80% used, THE Audit_Module SHALL return `disk.low-space` as `medium`, at least 90% as `high`, and at least 95% as `critical`. WHEN a filesystem of at least 1 GiB total size has less than 100 MiB available, THE Audit_Module SHALL treat that as `high`; smaller filesystems are assessed by percentage only. THE Audit_Module SHALL return one result per filesystem with the highest applicable severity. The used percentage is computed as `df` does, `ceil(used*100/(used+avail))`, and the same value SHALL be displayed and used for the thresholds.
3. THE Audit_Module SHALL assess only the meaningful filesystems returned by the Inventory_Module and SHALL add a limitation WHEN the filesystem list was truncated, returning `incomplete` and not `passed` for the disk rule in that case unless a finding exists.
4. WHEN a service unit has active state `failed`, THE Audit_Module SHALL return `svc.failed` as a `medium` finding for that unit, up to 20 findings, then add a limitation. Inactive, disabled and `unknown` units SHALL NOT produce a finding.
5. WHEN the service list is complete and no unit has active state `failed`, THE Audit_Module SHALL return one `svc.failed` result with Outcome `passed`. WHEN the service list is truncated and no failed unit was seen, THE Audit_Module SHALL return `incomplete`.
6. WHEN a socket listens on port 3306, 5432, 6379, 27017, 11211 or 9200, THE Audit_Module SHALL treat it as a potential database listener and return `net.database-exposed` as follows: Binding_Class `loopback` gives no finding; a `specific` address in a private or link-local range (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `169.254.0.0/16`, `fc00::/7`, `fe80::/10`) gives a `low` finding; `wildcard` gives a `medium` finding; any other `specific` address gives a `medium` finding with the address as evidence.
7. THE `net.database-exposed` result SHALL set `inferred` true unless the process name is visible and matches the expected service, SHALL state that the port listens on a non-loopback address, and SHALL NOT state that it is publicly reachable. THE Audit_Module SHALL NOT assign `high` solely from port and binding, and SHALL NOT scan externally.
8. WHEN the socket list is truncated and no database listener finding exists, THE Audit_Module SHALL return `net.database-exposed` as `incomplete`.
9. THE Audit_Module SHALL NOT include a CPU utilization rule, and SHALL define all thresholds as constants in `rules.rs`.

### Requirement 9: SSH configuration rules

**User Story:** As a server operator, I want SSH hardening checks based on the real configuration, so that I know the effective global settings.

#### Acceptance Criteria

1. WHEN the `ssh` Step runs, THE Audit_Module SHALL read `/etc/ssh/sshd_config` with a limited reader (not a full OpenSSH parser) and expand `Include` patterns with relative paths resolved under `/etc/ssh`, lexical file order, depth at most 4 and at most 32 files.
2. THE Audit_Module SHALL accept only include patterns that resolve to an absolute path matching `^/etc/ssh/[A-Za-z0-9._/*-]+$` without `..`. WHEN a pattern is unsupported, or the depth or file limit is reached, THE Audit_Module SHALL record a limitation and return `incomplete` for both SSH rules unless a finding already exists. THE Audit_Module SHALL NOT return `passed` after skipping an include.
3. THE Audit_Module SHALL treat keywords case-insensitively, accept `Keyword value` and `Keyword=value`, strip quotes, and use the first value seen.
4. WHEN a `Match` line is reached, THE Audit_Module SHALL stop collecting directives, record `matchBlocks: true`, and state that Match blocks are not fully assessed. WHEN `matchBlocks` is true, a result that would be `passed` SHALL be `incomplete`; a `finding` is kept.
5. IF any file in the include chain is unreadable, THEN THE Audit_Module SHALL return `unavailable` for both SSH rules.
6. WHEN the first global `PasswordAuthentication` value is `yes`, or the directive is absent in fully read files (OpenSSH default `yes`), THE Audit_Module SHALL return `ssh.password-auth` as a `medium` finding. WHEN it is `no`, THE Audit_Module SHALL return `passed` with a limitation that the result covers the global setting only, not per-user or Match settings, and SHALL NOT claim password authentication is disabled for every user.
7. WHEN the first global `PermitRootLogin` value is `yes`, THE Audit_Module SHALL return `ssh.root-login` as a `high` finding. WHEN it is `no`, as `passed`.
8. WHEN `PermitRootLogin` is `prohibit-password`, `without-password`, `forced-commands-only`, or absent in fully read files (OpenSSH default `prohibit-password`), THE Audit_Module SHALL return `ssh.root-login` as `passed` with a limitation, and SHALL describe the observed restriction ("root login allowed with public key only" or "root login allowed for forced commands only"). THE Audit_Module SHALL NOT describe these values as "root login disabled".
9. IF a keyword has an unrecognized or unsupported value, THEN THE Audit_Module SHALL return `error` for that rule and SHALL NOT treat it as a safe default.
10. THE SSH results SHALL state that they describe global configuration on disk, not effective per-user settings and not the running daemon.
11. THE SSH results SHALL contain only the two keyword lines and file names as evidence.

### Requirement 10: Firewall, updates and certificate rules

**User Story:** As a server operator, I want firewall, update and certificate checks with honest limits, so that unknowns are not presented as safe.

#### Acceptance Criteria

1. WHEN `/etc/ufw/ufw.conf` contains `ENABLED=yes`, THE Audit_Module SHALL return `fw.ufw-config` as `passed` with a limitation that effective state needs root.
2. WHEN `/etc/ufw/ufw.conf` contains `ENABLED=no`, THE Audit_Module SHALL return `fw.ufw-config` as a `low` finding.
3. IF `/etc/ufw/ufw.conf` is absent or unreadable, THEN THE Audit_Module SHALL return `fw.ufw-config` as `unavailable`.
4. WHEN `/var/lib/update-notifier/updates-available` reports security updates greater than 0, THE Audit_Module SHALL return `upd.security-pending` as a `medium` finding, and WHEN it reports only non-security updates, as `info`.
5. IF `/var/lib/update-notifier/updates-available` is absent, THEN THE Audit_Module SHALL return `upd.security-pending` as `unavailable`. WHEN it reports zero updates but the apt metadata is stale or its age is unknown, THE Audit_Module SHALL return `incomplete` and SHALL NOT return `passed` or describe the server as fully patched.
6. WHEN the age of `/var/lib/apt/periodic/update-success-stamp` exceeds 7 days, THE Audit_Module SHALL return `upd.metadata-stale` as a `low` finding, and as `unavailable` WHEN the stamp is missing.
7. WHEN `/var/run/reboot-required` exists, THE Audit_Module SHALL return `upd.reboot-required` as an `info` finding.
8. THE `updates` Step SHALL NOT run `apt update`, `apt list --upgradable` or `apt-check`, and every update result SHALL be labelled as based on cached update-notifier and apt metadata, not live repository state, with the cache age shown.
9. WHEN the `certificates` Step runs, THE Audit_Module SHALL find `ssl_certificate` directives in at most 32 nginx config files of at most 256 KB each and check at most 20 certificate paths. WHEN discovery hits a limit, a file is unreadable or a path is skipped, THE Audit_Module SHALL return `incomplete` for the certificate assessment unless a finding exists.
10. THE Audit_Module SHALL accept a certificate path only WHEN it is absolute, at most 255 characters, matches `^[A-Za-z0-9._/-]+$`, has no `..` component and starts with `/etc/letsencrypt/live/`, `/etc/letsencrypt/archive/`, `/etc/ssl/` or `/etc/nginx/`.
11. IF a certificate path is rejected, THEN THE Audit_Module SHALL skip it with a limitation and SHALL NOT pass it to a command.
12. WHEN a certificate has expired, THE Audit_Module SHALL return `tls.cert-expiry` as `high`, WHEN it expires in under 14 days as `high`, and WHEN it expires in under 30 days as `medium`.
13. IF no nginx configuration exists, THEN THE Audit_Module SHALL return `notApplicable` for `tls.cert-expiry`.
14. IF a certificate file is not readable, THEN THE Audit_Module SHALL return `unavailable` for that certificate.
15. THE `tls.cert-expiry` results SHALL cover only certificates referenced in inspected, readable nginx configuration, SHALL NOT claim a certificate is actively served, SHALL NOT read private key files or return whole configuration files, and SHALL NOT scan TLS endpoints.

### Requirement 11: Read-only boundary

**User Story:** As a server owner, I want Doctor to be unable to change my server, so that I can run it on production hosts.

#### Acceptance Criteria

1. THE Audit_Module and the extended Inventory_Module SHALL run only predefined `CommandSpec` commands through the Executor.
2. THE Audit_Module SHALL NOT run `sudo`.
3. THE Doctor SHALL register no module with a mutating or privileged operation.
4. THE Audit_Module SHALL validate server-derived values used in command arguments into typed values before use.
5. THE Audit_Module SHALL parse remote output in Rust and return no raw command output over IPC.
6. THE Doctor frontend SHALL render remote strings as text.
7. THE Audit_Module SHALL provide recommendations as text only and SHALL NOT execute them.

### Requirement 12: Scan workflow

**User Story:** As a server operator, I want to run, watch and cancel a scan, so that I stay in control of the work done on my server.

#### Acceptance Criteria

1. WHEN the user starts a Scan, THE Doctor SHALL read the Step order from `list_modules` and run the Steps one at a time.
2. WHILE a Scan is running, THE Doctor SHALL display progress as "step n of N".
3. WHEN the user cancels a Scan, THE Doctor SHALL call `cancel_operation`, start no further Step, keep completed Step results and label the Scan "Incomplete: cancelled before <step>".
4. IF the connection is lost during a Scan, THEN THE Doctor SHALL end the Scan as incomplete and mark results stale with their age.
5. IF a Step fails with a timeout, THEN THE Doctor SHALL record the failure, continue with the next Step and label the Scan incomplete.
6. WHEN a Scan completes, THE Doctor SHALL fill the Services, Network and Software pages from the Step evidence without collecting it again.
7. WHEN a server is removed, THE Doctor SHALL clear its results.
8. THE Doctor SHALL keep Scan results in memory only.
9. THE Doctor SHALL NOT retry a failed Step automatically.
10. WHEN a Scan starts, THE Doctor SHALL assign a new Scan_Id for that server, discard the previous results and ignore any response whose Scan_Id is not the current one, so that old responses cannot overwrite newer results and two Scans never mix.
11. THE Doctor SHALL key results by server id and endpoint (host and port), and SHALL display only results whose key matches the selected server.
12. WHEN the host or port of a server is changed, THE Doctor SHALL invalidate that server's results.
13. WHEN a server disconnects, THE Doctor SHALL keep completed Step results, mark them stale and the Scan incomplete.
14. THE Doctor SHALL show Scan_Status and Coverage separately and SHALL label partial results as partial.

### Requirement 13: Doctor navigation and UI states

**User Story:** As a server operator, I want clear pages for each kind of result, so that I can inspect evidence without ambiguity.

#### Acceptance Criteria

1. THE Doctor SHALL provide the pages Overview, Diagnostics, Services, Network, Software, Security and Reports, and no page for an unimplemented feature.
2. THE Overview page SHALL show scan controls, summary counts, the completeness label and the extended inventory including swap, filesystems and CPU usage.
3. THE Diagnostics page SHALL list all Check_Results with severity and Outcome filters and text search.
4. WHEN the user selects a Check_Result, THE Doctor SHALL show its evidence, explanation, recommendation, limitations, observation time and an `inferred` badge WHERE `inferred` is true.
5. THE Security page SHALL reuse the Diagnostics component, filtered to the categories `security`, `network` and `updates`.
6. THE Doctor SHALL show loading, `unavailable` with reason, and connection-failure states on every table and detail view.
7. THE `@ossiframe/ui` package SHALL gain only a severity badge.
8. THE `@ossiframe/client` types SHALL include the new `ServerInfo` fields as `number | null` WHERE a value can be missing.
9. THE Doctor UI SHALL present a professional diagnostics layout using existing shared UI components: summary cards (severity counts, Coverage, Scan_Status), a clear heading hierarchy, severity and Outcome filters, searchable tables, detail panels, a progress indicator, and clear `unavailable` and `incomplete` states. THE Doctor SHALL NOT add heavy dependencies, animations or charts.

### Requirement 14: Reporting

**User Story:** As a server operator, I want to review and export a report, so that I can share findings without exposing secrets.

#### Acceptance Criteria

1. WHEN the user requests a JSON report, THE Report_Builder SHALL produce JSON with `schemaVersion` 1, `generatedAt`, `server`, `scan`, `infrastructure`, `summary`, `results`, `findings`, `unavailable` and `limitations`.
2. THE JSON `server` object SHALL contain the profile name, port, hostname and OS and SHALL NOT contain a key path or username. THE JSON `scan` object SHALL contain Scan_Status, Coverage, timestamps and missing Steps, and THE report SHALL preserve all limitations.
3. WHERE host redaction is enabled, THE Report_Builder SHALL omit only the configured connection address (`host`). THE Doctor SHALL describe this option accurately and SHALL NOT call the report anonymized, because hostnames, service names, package versions, ports, process names and file paths remain.
4. WHEN the user requests an HTML report, THE Report_Builder SHALL produce one self-contained file with every dynamic value escaped by one function, no scripts, no remote assets, and a `Content-Security-Policy` meta tag of `default-src 'none'; style-src 'unsafe-inline'`.
5. WHEN the user opens the Reports page, THE Doctor SHALL show a preview from `build_report` before any export, and SHALL tell the user that the report contains server-derived data to review before sharing.
6. THE `build_report` and `save_report` requests SHALL be typed structures validated in Rust with unknown fields denied, at most 4 MiB, at most 8 Steps, at most 100 Check_Results per Step and bounded string lengths. THE `build_report` command SHALL perform no I/O.
7. WHEN the user exports a report, THE `save_report` command SHALL rebuild the report from the same request, open a native save dialog from Rust, write only the chosen path and return `false` IF the user cancels. THE native dialog SHALL confirm overwriting an existing file.
8. THE Report_Commands SHALL NOT accept a file path or file content from the frontend.
9. THE Doctor capability file SHALL list only the shared commands plus `build_report` and `save_report` and SHALL contain no `dialog:` or `fs:` permission.
10. THE Report_Builder SHALL NOT emit passwords, passphrases, key material or configuration content beyond the specific evidence lines.
11. IF a report save fails, THEN THE Doctor SHALL show a short error on the Reports page and change nothing else.
12. THE Doctor SHALL build the preview and the export from one immutable assessment snapshot and one redaction setting, and THE Report_Builder SHALL be deterministic so that the preview and the exported file differ only in `generatedAt`.

### Requirement 15: Generator catalog and documentation

**User Story:** As an app developer, I want `linux-audit` available in generated apps, so that I can reuse the audit logic in my own app.

#### Acceptance Criteria

1. THE Generator catalog SHALL contain a `linux-audit` entry with crate `ossiframe-mod-linux-audit`, type path `ossiframe_mod_linux_audit::LinuxAudit`, directory `modules/linux-audit` and no UI directory.
2. WHEN a user selects `linux-audit`, THE Generator SHALL add `linux-inventory` through the manifest dependency.
3. THE Generator SHALL NOT add a Doctor UI to generated apps.
4. THE `docs/developer-guide.md` SHALL list `linux-inventory` and `linux-audit` in the module catalog and update the `--modules` text.
5. THE `docs/architecture.md` SHALL name the auditing box `linux-audit`.

### Requirement 16: Testing

**User Story:** As a maintainer, I want small tests on high-risk logic, so that parsers, rules and reports are trusted without a heavy test suite.

#### Acceptance Criteria

1. THE parsers for `ss`, `systemctl` JSON, `dpkg-query`, `/proc/stat`, `df`, `updates-available` and `openssl -enddate` SHALL have plain unit tests with inline strings, including malformed and partial input.
2. THE sshd reader SHALL have unit tests for first-value-wins across includes, `Keyword=value`, case-insensitivity, `Match` stop, unreadable include, depth and file limits, and hostile include patterns.
3. THE rules SHALL have unit tests for each threshold boundary, for missing data returning `unavailable` and not `passed`, and for `inferred` on database exposure.
4. THE certificate path validator SHALL have unit tests for traversal, variables, quotes, newlines, a leading `-`, and wrong prefix.
5. THE Steps SHALL have unit tests with a fake Executor for argument rejection, permission denied, cancel and connection-loss propagation, and for running no command outside the fixed list.
6. THE Report_Builder SHALL have unit tests for HTML escaping of `<script>` in hostnames and evidence, host redaction, absence of secret-like strings, and the JSON fields.
7. THE Doctor SHALL have a unit test that the capability file and `build.rs` command lists match and contain no `dialog:` or `fs:` entries.
8. THE Generator SHALL have unit tests that `linux-audit` resolves with its dependency and that `OSSIFRAME-LICENSE` is written with the MIT text.
9. THE S03 test suite SHALL NOT use a property-testing framework, fixture suites or end-to-end suites.

### Requirement 17: Real-server verification

**User Story:** As a maintainer, I want results checked against a real server, so that Doctor claims only what was observed.

#### Acceptance Criteria

1. THE verification SHALL run only against the Verification_Server.
2. WHEN live verification starts, THE maintainer SHALL confirm the host fingerprint through the Lightsail browser console before trusting the key, and `ssh-keyscan` output alone SHALL NOT count as trust. Until then, only live verification SHALL be blocked; local implementation, unit tests and quality gates SHALL proceed.
3. THE verification SHALL run only read-only Doctor operations.
4. THE verification SHALL compare Doctor results with real command output for memory, filesystems, listening sockets including one without a visible process, installed packages, `sshd_config` and `sshd_config.d`, `updates-available`, and a failed or inactive unit WHERE one exists.
5. THE verification SHALL NOT induce or fabricate findings and SHALL record that no findings exist WHERE the server has none.
6. THE verification SHALL check cancel mid-scan, disconnect mid-scan with the incomplete label, and `unavailable` Outcomes for root-only data.
7. THE verification SHALL export JSON and HTML, check the schema, escaping and redaction toggle, and search the files for password, passphrase and key material.
8. THE verification SHALL generate an app with `--modules linux-audit`, confirm `linux-inventory` is added, build it, and run one `runOperation` call through the public client interface.
9. THE verification SHALL run all Quality_Gates.
10. THE verification evidence SHALL be recorded, sanitized as in Requirement 1, in `docs/verification/s03-ossiframe-doctor.md`, and items not run SHALL be listed as not verified.
11. THE verification SHALL NOT change the server, and SHALL record checks that could not be verified as not verified.

### Requirement 18: S03 completion criteria

**User Story:** As the project owner, I want an explicit definition of done, so that S03 is closed only when all of its goals are met on a real server.

#### Acceptance Criteria

1. THE S03 SHALL be complete only WHEN Doctor is an independently useful infrastructure diagnostics application.
2. THE S03 SHALL be complete only WHEN Doctor connects to actual authorized Linux infrastructure.
3. THE S03 SHALL be complete only WHEN existing server connection management remains functional.
4. THE S03 SHALL be complete only WHEN reusable infrastructure modules provide actual discovery data.
5. THE S03 SHALL be complete only WHEN diagnostic checks produce meaningful, evidence-based results.
6. THE S03 SHALL be complete only WHEN findings distinguish severity and verification limitations.
7. THE S03 SHALL be complete only WHEN services, network and software information are available through the application.
8. THE S03 SHALL be complete only WHEN diagnostic scans support progress, cancellation and partial results.
9. THE S03 SHALL be complete only WHEN JSON report export works.
10. THE S03 SHALL be complete only WHEN Doctor performs no unauthorized server mutations.
11. THE S03 SHALL be complete only WHEN reusable modules can be consumed outside Doctor.
12. THE S03 SHALL be complete only WHEN production builds and relevant quality checks pass.
13. THE S03 SHALL be complete only WHEN real-server verification is recorded honestly.

## Non-Goals

The following are out of scope for S03:

- Provisioning and package installation.
- Config editing.
- Service, firewall or SSH mutation.
- Remediation of findings.
- AI diagnostics.
- Continuous monitoring and alerting.
- A backend, accounts or multi-user support.
- Dynamic plugins.
- Ossiframe Deploy.
- PDF reports.
- A security score.
- Privileged (`sudo`) reads.
