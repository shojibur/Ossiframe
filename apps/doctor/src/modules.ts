import type { ModuleUi } from "@ossiframe/ui";
import { clearServer as clearInventory, loadServerInfo, markStale as markInventoryStale } from "../../../modules/linux-inventory/ui";
import { diagnosticsUi, networkUi, overviewUi, reportsUi, securityUi, servicesUi, softwareUi } from "./pages";
import { clearServer as clearScan, markStale as markScanStale, reconcileEndpoint } from "./scan-store";

/** Module pages shown by Doctor. Keep in step with the modules registered in Rust. */
export const pages: ModuleUi[] = [overviewUi, diagnosticsUi, servicesUi, networkUi, softwareUi, securityUi, reportsUi];

export { loadServerInfo };

/** Connection data is out of date: mark both the inventory and the assessment as stale. */
export function markStale(serverId: string) {
  markInventoryStale(serverId);
  markScanStale(serverId);
}

/** A connect succeeded. Results from another endpoint are dropped. */
export function onConnected(serverId: string) {
  void loadServerInfo(serverId);
  void reconcileEndpoint(serverId);
}

export function clearServer(serverId: string) {
  clearInventory(serverId);
  clearScan(serverId);
}
