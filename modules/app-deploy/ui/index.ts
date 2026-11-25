import type { ModuleUi } from "@ossiframe/ui";
import { AppsPage } from "./AppsPage";
import { clearServer } from "./store";

/** Optional UI for the `app-deploy` module: the Applications tab. */
export const moduleUi: ModuleUi = { id: "app-deploy/applications", title: "Applications", Page: AppsPage };

/** Optional lifecycle hooks that apps wire to the server list events. */
export const hooks = {
  onRemoved: clearServer,
};
