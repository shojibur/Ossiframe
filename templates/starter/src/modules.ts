import type { ModuleUi } from "@ossiframe/ui";
// Generator insertion point: one `import * as <alias> from "./modules/<id>";` per selected module that has a UI folder.
{{module_ui_imports}}

/** What a module UI folder may export: `moduleUi` (one page) or `moduleUis` (several pages), and optional `hooks`. */
interface PageHooks {
  onStale?: (serverId: string) => void;
  onConnected?: (serverId: string) => void;
  onRemoved?: (serverId: string) => void;
}
interface ModuleUiExports {
  moduleUi?: ModuleUi;
  moduleUis?: ModuleUi[];
  hooks?: PageHooks;
}

// Generator insertion point: one `<alias>,` per imported module UI, in module order.
const entries: ModuleUiExports[] = [
  {{module_ui_list}}
];

/** Module pages shown as tabs. With no modules the app shows connection info only. */
export const pages: ModuleUi[] = entries.flatMap((e) => e.moduleUis ?? (e.moduleUi ? [e.moduleUi] : []));

export const onStale = (id: string) => entries.forEach((e) => e.hooks?.onStale?.(id));
export const onConnected = (id: string) => entries.forEach((e) => e.hooks?.onConnected?.(id));
export const onRemoved = (id: string) => entries.forEach((e) => e.hooks?.onRemoved?.(id));
