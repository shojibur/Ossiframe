# Ossiframe

**One foundation. Endless infrastructure tools.**

Ossiframe is an open-source, local-first framework for building desktop and command-line applications that manage remote Linux servers over SSH. It provides what every infrastructure tool needs: SSH connections with host key pinning, OS-keychain credentials, a safe remote execution engine, an operation model that handles unknown outcomes, compile-time modules, and a shared desktop UI foundation.

Two applications are built on it:

- **Ossiframe Deploy**: provision fresh Ubuntu servers and deploy Laravel, PHP and static applications.
- **Ossiframe Doctor**: discover, audit and diagnose existing servers, read-only.

Developers can generate their own applications from the starter template, choose modules, and add their own without changing the core.

> **Status: early development.**
> - Doctor is verified against a real Ubuntu 24.04 server.
> - Deploy's provisioning ran one full install on a fresh Ubuntu 24.04 server with every component verified. Its recovery and locking checks are not run yet.
> - Application deployment is implemented but has not run on a real server yet.
>
> See [docs/known-limitations.md](docs/known-limitations.md) and [docs/verification/](docs/verification/).

## Install (macOS Apple Silicon)

No release is published yet. Once one is, download the apps from [GitHub Releases](https://github.com/shojibur/Ossiframe/releases) and follow [docs/install.md](docs/install.md). Builds are ad-hoc signed, so macOS asks you to confirm the first launch.

## Run from source

Requirements: Rust (stable, via rustup), Node.js 24, pnpm 11, Xcode Command Line Tools.

```bash
pnpm install
pnpm doctor                                    # Doctor in development mode
pnpm --filter @ossiframe/deploy tauri dev      # Deploy in development mode
# release builds of both apps (.app, .dmg and SHA256SUMS):
tools/build-release.sh dist
```

Only connect to servers you are authorized to access. Deploy changes servers as root after you approve a plan; use it only on servers you can rebuild.

To generate your own app from the starter template, see the [developer quick-start](docs/developer-guide.md).

## Repository layout

| Path                      | Contents                                                                                |
| ------------------------- | --------------------------------------------------------------------------------------- |
| `crates/`                 | Skeleton crates: `ossiframe-core`, `ossiframe-ssh`, `ossiframe-desktop`                 |
| `packages/`               | `@ossiframe/ui`, `@ossiframe/client`                                                    |
| `modules/`                | Optional modules: `linux-inventory`, `linux-audit`, `ubuntu-provisioning`, `app-deploy` |
| `apps/deploy/`            | Ossiframe Deploy                                                                        |
| `apps/doctor/`            | Ossiframe Doctor                                                                        |
| `templates/starter/`      | Starter template                                                                        |
| `tools/`                  | Project generator (`ossiframe-cli`) and release scripts                                 |
| `examples/custom-module/` | Extension example                                                                       |
| `docs/`                   | Install, product, architecture, security, decisions, roadmap, verification              |
| `.github/workflows/`      | CI and release workflows                                                                |
| `.kiro/`                  | Kiro steering, specs, hooks and skills used to build the project                        |

## Documentation

See [docs/README.md](docs/README.md).

## License

[MIT](LICENSE)
