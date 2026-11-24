---
inclusion: always
---
# Structure

```text
.kiro/                       steering, specs, hooks, skills (always tracked, never git-ignored)
crates/ossiframe-core/       shared types, public contracts, operation and module metadata
crates/ossiframe-ssh/        auth, host verification, sessions, remote execution
crates/ossiframe-desktop/    Tauri integration, SQLite, keychain, IPC support
packages/ui/                 @ossiframe/ui (shared React components)
packages/client/             @ossiframe/client (typed IPC wrappers)
modules/<name>/              optional infrastructure modules (ossiframe-mod-<name>)
apps/deploy/                 Ossiframe Deploy
apps/doctor/                 Ossiframe Doctor
templates/starter/           starter template (S02)
tools/ossiframe-cli/         repository-local project generator
examples/custom-module/      extension example
docs/                        product, architecture, security, decisions, roadmap, compatibility
```

- Create a directory or crate only when it gets real content.
- Add a new crate only for a concrete dependency, build or reuse need.
- Rust: kebab-case crate names, snake_case modules. TypeScript packages: `@ossiframe/<name>`.
