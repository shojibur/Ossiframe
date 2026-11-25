# Roadmap

Six specifications, implemented one at a time. Each ends with something that works against a real Ubuntu 24.04 VPS.

| Spec | Delivers | Depends on | Status |
|---|---|---|---|
| S01 Skeleton Foundation | Monorepo, Core, SSH auth, host key pinning, keychain, server metadata, basic remote execution, real server information in a minimal desktop app | none | Done: essential live verification passed; open checks in `docs/verification/` |
| S02 Modules and Project Generator | Public module interfaces, explicit registration, starter template, module selection, generator, standalone builds, custom module example | S01 | Done: all 12 completion criteria met (some with notes); see `docs/verification/s02-modules-and-generator.md` |
| S03 Ossiframe Doctor | Independent app, read-only discovery, inventory, security checks, service diagnostics, evidence-based findings, report export | S02 | Closed: all 13 completion criteria met (8 Met, 5 with notes); see `docs/verification/s03-ossiframe-doctor.md`. Standalone_Mode verified from published rev `abc2260`. The later fixes (tasks 11.1 to 11.4) are local and not yet published. A visible `unavailable` outcome and password authentication remain unverified |
| S04 Infrastructure Provisioning | Fresh Ubuntu preflight, package installation, NGINX, PHP, Composer, MySQL, verification and recovery | S02 | In progress: implemented locally, live verification pending (no mutation has run on a real server). Deferred to S05: configuration-write primitive and the stdin field on `CommandSpec` |
| S05 Ossiframe Deploy | Independent app, Laravel lifecycle, Git deployment, isolation, environment, database association, deployment history, health checks | S04 | Not started |
| S06 Distribution and Verification | Independent desktop builds, generated project verification, GitHub Actions release, installers, developer docs, known limitations | S03, S05 | Not started |

Doctor comes before provisioning because it is read-only: it exercises the module system and executor without needing mutation, locking or recovery.

Module contracts from S02 are treated as stable while S03 and S04 are built. Changes to them go through an explicit spec update.

Capabilities beyond the first release are listed in [product/requirements.md](product/requirements.md).

## Known risks

| Risk | Mitigation |
|---|---|
| russh streaming, cancellation or disconnect handling is harder than expected | Validated first in S01; report and revisit ADR-0003 if it fails |
| Keychain prompts on unsigned macOS development builds | Not yet verified. The S01 checklist item "Keychain behavior with the unsigned build (prompts, locked keychain)" is still open in `docs/verification/s01-skeleton-foundation.md` |
| Two Tauri apps sharing crates and packages in one workspace | Shared crates and packages set up in S01 and S02; second app (Deploy) arrives in S05 |
| Generated projects accidentally depend on monorepo paths | Build a generated project outside the repository in S02 and S06 |
| Fresh VPS dpkg lock held by unattended-upgrades | Lock detection with bounded wait in S04 |
| Unsigned macOS builds blocked by Gatekeeper | Installation instructions; notarization decision in S06 |
