# Requirements Document

## Introduction

S06 makes Ossiframe Doctor and Ossiframe Deploy installable from GitHub Releases for macOS Apple Silicon, runs the required checks in CI, verifies generated projects in standalone mode, and documents installation, development and known limitations. These requirements are derived from `design.md`.

## Glossary

- **CI_Workflow**: `.github/workflows/ci.yml`, run on pushes to `main` and on pull requests.
- **Release_Workflow**: `.github/workflows/release.yml`, run on a `v*` tag or manually as a dry run.
- **Draft_Release**: A GitHub Release created as a draft by the Release_Workflow. Only the user publishes it.
- **Ad_Hoc_Signature**: The macOS pseudo-identity `-` used when no Apple Developer certificate is configured.
- **Generated_Project_Check**: The CI job that generates apps outside the checkout with `--rev` and builds them.
- **Known_Limitations**: `docs/known-limitations.md`.

## Requirements

### Requirement 1: Continuous integration

**User Story:** As a maintainer, I want every push and pull request checked, so that regressions are caught before release.

#### Acceptance Criteria

1. THE CI_Workflow SHALL run `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` with and without `--all-features`, `cargo test --workspace --all-features`, `pnpm typecheck` and `pnpm lint`.
2. THE CI_Workflow SHALL check Doctor and Deploy individually and SHALL fail if Doctor's dependency tree contains `ossiframe-mod-app-deploy` or `ossiframe-mod-ubuntu-provisioning`.
3. THE CI_Workflow SHALL run `cargo audit --deny warnings` and `pnpm audit`, and SHALL fail on any finding absent from the reviewed list in `.cargo/audit.toml`, which states a reason for each entry.
4. THE CI_Workflow SHALL use workflow-level permission `contents: read`.
5. Every third-party action SHALL be pinned to a full commit SHA.

### Requirement 2: Generated project verification

**User Story:** As a framework user, I want proof that generated projects build from a published commit, so that I can rely on standalone mode.

#### Acceptance Criteria

1. WHEN a commit is pushed to `main`, THE Generated_Project_Check SHALL generate a read-only app and a provisioning app with `--rev` set to that commit, outside the checkout.
2. THE Generated_Project_Check SHALL run `cargo check`, `pnpm install`, `pnpm typecheck` and the frontend build for each generated app.
3. THE Generated_Project_Check SHALL fail if the read-only app's handler, `build.rs` or capability contains a provisioning command or permission.

### Requirement 3: Release builds

**User Story:** As a user, I want to download both apps with checksums, so that I can install them and verify the download.

#### Acceptance Criteria

1. WHEN a `v*` tag is pushed, THE Release_Workflow SHALL fail unless the tag equals the version in the workspace `Cargo.toml`, both `tauri.conf.json` files and both app `package.json` files.
2. THE Release_Workflow SHALL run the CI checks before building.
3. THE Release_Workflow SHALL build Doctor and Deploy for macOS Apple Silicon as `.dmg` and zipped `.app`, and SHALL write one `SHA256SUMS` file covering every artifact.
4. THE Release_Workflow SHALL create a Draft_Release and SHALL never publish it.
5. WHEN run manually, THE Release_Workflow SHALL upload the artifacts to the workflow run and SHALL create no release.
6. THE Release_Workflow SHALL grant `contents: write` only to the job that creates the release.

### Requirement 4: Signing

**User Story:** As a user on Apple Silicon, I want apps that macOS can open, without the project needing a paid account.

#### Acceptance Criteria

1. WHEN no Apple certificate secret is configured, THE build SHALL use the Ad_Hoc_Signature.
2. WHEN the Apple certificate and notarization secrets are configured, THE Release_Workflow SHALL sign with the Developer ID identity and notarize.
3. Secrets SHALL be passed only to the steps that need them and SHALL never be printed.

### Requirement 5: Documentation

**User Story:** As a user or developer, I want accurate install, development and limitation docs.

#### Acceptance Criteria

1. `docs/install.md` SHALL describe download, checksum verification, opening an ad-hoc signed app, keychain prompts as observed, local data location and uninstalling, including keychain items.
2. `docs/developer-guide.md` SHALL cover Deploy, the provisioning opt-in, the app-deploy module and its app types, and the Generated_Project_Check.
3. Known_Limitations SHALL list every unverified item, deferred feature and platform limit recorded in `docs/verification/` and the spec notes, each with its source, and SHALL state that app-deploy is not offered by the generator.
4. The starter module skill SHALL document both `moduleUi` and `moduleUis`.
5. No doc SHALL claim behavior that was not observed.

### Requirement 6: Verification

**User Story:** As a maintainer, I want the distribution path proven before the first release.

#### Acceptance Criteria

1. A CI_Workflow run on `main` SHALL pass, and its run link SHALL be recorded in `docs/verification/s06-distribution-and-verification.md`.
2. A manual Release_Workflow dry run SHALL produce both apps and `SHA256SUMS`, and the user SHALL verify the checksum and open both apps on their Mac, recording Gatekeeper and keychain behavior.
3. THE first version tag SHALL be created only after the essential S04 and S05 live checks pass and the user approves.
