---
inclusion: always
---
# Technology

Approved starting stack (see `docs/decisions/`):

- Desktop: Tauri 2. Backend: Rust (Tokio). Frontend: React + TypeScript, Tailwind CSS, shadcn/ui.
- SSH: russh, used only inside `ossiframe-ssh`. Its behavior is being validated in S01; it is not verified yet.
- Credentials: OS keychain via a maintained crate (keyring), used only inside `ossiframe-desktop`.
- Persistence: SQLite, inside `ossiframe-desktop`.
- Workspaces: Cargo (Rust), pnpm (frontend). Monorepo.
- Server side: SSH plus native Ubuntu tools. No remote agent.
- Server targets: Ubuntu 24.04 first; Ubuntu 26.04 only after verification. Database: MySQL first, PostgreSQL later. Web server: NGINX. Framework: Laravel first.
- Primary dev platform: macOS Apple Silicon. Keep the app light enough for 8 GB machines.

Rules:
- Never implement cryptography or SSH protocol code.
- Check current official docs for Tauri 2 and russh APIs instead of relying on memory.
- Add a dependency only when it is needed. It must be maintained and permissively licensed.
- If a chosen technology shows a material limitation or security problem, report it and revisit the ADR. Do not silently switch.
