import type { ModuleUi } from "@ossiframe/ui";
import { Overview } from "./Overview";
import { clearServer, loadServerInfo, markStale } from "./info-store";

export { clearServer, loadServerInfo, markStale } from "./info-store";

/** Optional UI for the `linux-inventory` module. `id` equals the Rust module id. */
export const moduleUi: ModuleUi = {
  id: "linux-inventory",
  title: "Overview",
  Page: Overview,
};

/** Optional lifecycle hooks that generated apps wire to the server list events. */
export const hooks = {
  onStale: markStale,
  onConnected: (serverId: string) => void loadServerInfo(serverId),
  onRemoved: clearServer,
};
