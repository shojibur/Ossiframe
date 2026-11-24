import type { ModuleUi } from "@ossiframe/ui";
import { moduleUi as linuxInventory } from "../../../modules/linux-inventory/ui";

/** Module pages shown by Doctor. Keep in step with the modules registered in Rust. */
export const pages: ModuleUi[] = [linuxInventory];

export { clearServer, loadServerInfo, markStale } from "../../../modules/linux-inventory/ui";
