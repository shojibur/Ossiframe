---
inclusion: always
---
# Product

Ossiframe is an open-source (MIT), local-first framework for building desktop and CLI applications that manage remote Linux servers over SSH. It is a framework first. Its first two applications are:

- **Ossiframe Deploy**: provisions servers and deploys applications (Laravel first).
- **Ossiframe Doctor**: read-only discovery, auditing and diagnostics, usable on servers Deploy never touched.

Layers: Core (reusable skeleton) → optional Modules → Applications. Third-party developers generate their own apps from the starter template and add modules without changing Core.

Principles:
- No required backend, account, subscription or AI provider. Hosted apps keep running if Ossiframe is closed or uninstalled.
- Servers are general-purpose. Never tie a server to one runtime; runtimes are independent capabilities.
- Real functionality only. Mock data only in labeled demo mode or isolated UI work.
- Claim only what has been verified on a real server.
- Do not copy Ploi or Laravel Forge code, branding or assets.

Details: `docs/product/vision.md`, `docs/product/requirements.md`, `docs/roadmap.md`.
