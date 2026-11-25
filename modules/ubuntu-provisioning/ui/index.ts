import type { ModuleUi } from "@ossiframe/ui";
import { HistoryPage } from "./HistoryPage";
import { OperationPage } from "./OperationPage";
import { PlanPage } from "./PlanPage";
import { PreflightPage } from "./PreflightPage";
import { SoftwarePage } from "./SoftwarePage";
import { clearServer, markStale } from "./store";

export { clearServer, markStale } from "./store";

/**
 * Optional UI for the `ubuntu-provisioning` module, one entry per tab in the order
 * Preflight, Software, Plan, Operation, History. Ids start with the Rust module id and
 * are unique so each page can be a tab.
 */
export const moduleUis: ModuleUi[] = [
  { id: "ubuntu-provisioning/preflight", title: "Preflight", Page: PreflightPage },
  { id: "ubuntu-provisioning/software", title: "Software", Page: SoftwarePage },
  { id: "ubuntu-provisioning/plan", title: "Plan", Page: PlanPage },
  { id: "ubuntu-provisioning/operation", title: "Operation", Page: OperationPage },
  { id: "ubuntu-provisioning/history", title: "History", Page: HistoryPage },
];

/** Optional lifecycle hooks that apps wire to the server list events. */
export const hooks = {
  onStale: markStale,
  onRemoved: clearServer,
};
