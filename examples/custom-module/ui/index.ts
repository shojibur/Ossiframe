import type { ModuleUi } from "@ossiframe/ui";
import { Ports } from "./Ports";

/** Optional UI for the `ports` example module. `id` equals the Rust module id. */
export const moduleUi: ModuleUi = {
  id: "ports",
  title: "Ports",
  Page: Ports,
};
