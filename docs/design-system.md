# Design System

A professional, desktop-style interface for developers. Keep it simple until the first workflow works.

## Rules

- Layout: left sidebar navigation, top bar with the selected server and connection state, main content, collapsible bottom panel for operation output.
- Dense, readable tables and key-value panels. Consistent spacing (4 px scale). System font; monospace for commands, paths and output.
- Light and dark themes from shared tokens; follow the OS setting.
- Status always uses icon plus label, not color alone. Meet WCAG AA contrast.
- Distinct, honest states: loading, empty, disconnected, permission denied, failed, unsupported, stale (with age).
- Never show success before the result is verified. Show the real exit code and stderr excerpt on failure.
- Mutations show the plan before approval. Destructive actions need typed confirmation.
- Keyboard navigation and visible focus throughout.
- No heavy animations or visual effects.

## Stack

React, TypeScript, Tailwind CSS, shadcn/ui, Lucide icons. Heavy components (terminal, editor, graph, charts) load only on the pages that use them.

## Shared components (`@ossiframe/ui`)

Built as needed: app shell, server list, add-server dialog, host key trust dialog, connection status, key-value panel, resource table, output viewer (text only), confirmation dialog, error and empty states.

## Navigation

- **Deploy:** Servers, Applications, Operation history, Settings. Per server: Overview, Applications, Provisioning, then more sections as they ship. Per application: Overview, Deployments, Environment, Database, then more.
- **Doctor:** Servers, Reports, Settings. Per server: Overview, Findings, Inventory, then diagnostics sections as they ship. No "apply fix" actions.
- **Starter app:** Servers and one page per selected module.

Do not copy Ploi or Forge layouts, names, icons or assets.
