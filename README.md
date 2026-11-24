# Ossiframe

**One foundation. Endless infrastructure tools.**

Ossiframe is an open-source, local-first framework for building desktop and command-line applications that manage remote Linux servers over SSH. It provides what every infrastructure tool needs: SSH connections with host key pinning, OS-keychain credentials, a safe remote execution engine, an operation model that handles unknown outcomes, compile-time modules, and a shared desktop UI foundation.

Two applications are built on it:

- **Ossiframe Deploy**: provision fresh Ubuntu servers and deploy Laravel applications.
- **Ossiframe Doctor**: discover, audit and diagnose existing servers, read-only.

Developers can generate their own applications from the starter template, choose modules, and add their own without changing the core.

> **Status: early development.** Ossiframe Doctor (S01) connects to a server over SSH with host key pinning and shows basic system information. It builds and runs locally on macOS Apple Silicon, but has not yet been verified against a real server. See [docs/verification/s01-skeleton-foundation.md](docs/verification/s01-skeleton-foundation.md).

## Run locally (macOS Apple Silicon)

Requirements: Rust (stable, via rustup), Node.js 24, pnpm 11, Xcode Command Line Tools.

```bash
pnpm install
pnpm doctor          # development mode
# or a release build:
pnpm --filter @ossiframe/doctor tauri build
```

Only connect to servers you are authorized to access.

## Repository layout (planned)

| Path | Contents |
|---|---|
| `crates/` | Skeleton crates: `ossiframe-core`, `ossiframe-ssh`, `ossiframe-desktop` |
| `packages/` | `@ossiframe/ui`, `@ossiframe/client` |
| `modules/` | Optional infrastructure modules |
| `apps/deploy/` | Ossiframe Deploy |
| `apps/doctor/` | Ossiframe Doctor |
| `templates/starter/` | Starter template |
| `tools/ossiframe-cli/` | Project generator |
| `examples/custom-module/` | Extension example |
| `docs/` | Product, architecture, security, decisions, roadmap |
| `.kiro/` | Kiro steering, specs, hooks and skills used to build the project |

## Documentation

See [docs/README.md](docs/README.md).

## License

[MIT](LICENSE)
