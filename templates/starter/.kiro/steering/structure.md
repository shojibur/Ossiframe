---
inclusion: always
---
# Structure

```text
src/                     React app: App.tsx, modules.ts (module pages), main.tsx
src/modules/<id>/        UI copied from a selected module (optional per module)
src-tauri/src/lib.rs     module registry and Tauri setup
src-tauri/build.rs       command list for the Tauri permission manifest
src-tauri/capabilities/  main.json, grants only the shared commands
modules/<id>/            your own module crates (Rust)
packages/ui, client      vendored @ossiframe packages
.kiro/                   steering and skills for this app
```

- Do not edit `packages/` unless you intend to diverge from Ossiframe. Changes there are not synced anywhere.
- Add a module as a Rust crate that depends on `ossiframe-core` only. Register it explicitly in `src-tauri/src/lib.rs`. Use the `ossiframe-module` skill.
- Do not add new Tauri commands for module work. Modules are reached through the shared `run_operation` command.
- Keep `capabilities/main.json` and the command list in `build.rs` in sync. The unit tests in `lib.rs` check this.
- Rust: kebab-case crate names, snake_case modules. Create a directory or crate only when it has real content.
