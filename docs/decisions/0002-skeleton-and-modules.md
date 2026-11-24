# ADR-0002: Reusable skeleton and module boundaries

Status: Accepted (2026-11-24)

## Decision
- Three skeleton crates: `ossiframe-core` (types, contracts, operation and module metadata), `ossiframe-ssh` (auth, host verification, sessions, execution), `ossiframe-desktop` (Tauri integration, SQLite, keychain, IPC helpers). Two frontend packages: `@ossiframe/ui`, `@ossiframe/client`.
- New crates only for a concrete dependency, build or reuse need.
- Dependency direction App → Modules → Core. Deploy and Doctor never depend on each other.
- Modules are trusted Rust crates registered explicitly in each app's entry point. A module declares ID, version, dependencies and provided capabilities. The registry rejects missing dependencies and cycles and produces a deterministic order.
- No dynamic plugin loading, no plugin sandbox, no RBAC for modules.
- Public contracts are added only when Deploy, Doctor or the starter needs them.

## Rationale
Few crates keep builds and versioning simple. Explicit registration is readable for third-party developers. Trusted compile-time modules do not justify a sandbox.

## Consequences
- Adding a module means adding a dependency and one registration line, then rebuilding.
- The manifest is metadata, not a security boundary. Real boundaries are the executor's privilege checks, which modules an app includes, and which IPC commands it exposes.
- A dependency check (Cargo metadata script, later run by a Kiro hook and CI) keeps Core free of module and app dependencies.
