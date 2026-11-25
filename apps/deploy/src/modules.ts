import type { ModuleUi } from "@ossiframe/ui";
import { hooks as inventoryHooks, moduleUi as inventoryUi } from "../../../modules/linux-inventory/ui";
import { hooks as provisioningHooks, moduleUis as provisioningUis } from "../../../modules/ubuntu-provisioning/ui";
import { hooks as appHooks, moduleUi as appUi } from "../../../modules/app-deploy/ui";

/** Module pages shown by Deploy. Keep in step with the modules registered in Rust. */
export const pages: ModuleUi[] = [inventoryUi, ...provisioningUis, appUi];

/** Combined lifecycle hooks of the registered modules. */
export function markStale(serverId: string) {
  inventoryHooks.onStale(serverId);
  provisioningHooks.onStale(serverId);
}

export function onConnected(serverId: string) {
  inventoryHooks.onConnected(serverId);
}

export function clearServer(serverId: string) {
  inventoryHooks.onRemoved(serverId);
  provisioningHooks.onRemoved(serverId);
  appHooks.onRemoved(serverId);
}
