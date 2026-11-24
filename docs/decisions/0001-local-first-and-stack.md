# ADR-0001: Local-first architecture and technology stack

Status: Accepted (2026-11-24)

## Decision
- Local desktop apps that talk to servers over SSH. No hosted backend, account, subscription, required AI provider or server agent.
- Anything that must keep running while the app is closed (cron, workers, certificate renewal) is configured on the server with native tools.
- Stack: Tauri 2, Rust (Tokio), React + TypeScript, Tailwind CSS, shadcn/ui, SQLite, OS keychain.
- Monorepo with a Cargo workspace and a pnpm workspace. MIT license.
- Primary development platform: macOS Apple Silicon.

## Rationale
Smallest trust surface, no operating cost for users, and infrastructure keeps working without Ossiframe. Tauri uses the system WebView and has a capability system for limiting frontend access, which keeps memory use and attack surface low.

## Consequences
- No continuous monitoring or push notifications while the app is closed.
- Each desktop platform's WebView (WKWebView, WebView2, WebKitGTK) needs separate verification.
- Tauri capabilities do not protect against bugs in our own Rust commands, so commands must validate input themselves.

## Unverified assumptions
- Idle memory of a built app is acceptable on an 8 GB machine. Measure in S01.
- Two Tauri apps can share crates and frontend packages in one workspace without friction. Confirm in S01 and S03.
