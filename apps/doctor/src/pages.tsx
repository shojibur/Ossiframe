// Doctor pages. Diagnostics and Security are the same FindingsView; Security only presets the
// categories. Navigation order is wired in modules.ts.
import type { ModuleUi } from "@ossiframe/ui";
import type { Category } from "./audit-types";
import { AUDIT_MODULE_ID } from "./audit-types";
import { FindingsView } from "./FindingsView";
import { NetworkPage, ServicesPage, SoftwarePage } from "./evidence-pages";
import { DoctorOverview } from "./OverviewPage";

import { ReportsPage } from "./ReportsPage";

const SECURITY_CATEGORIES: Category[] = ["security", "network", "updates"];

export const overviewUi: ModuleUi = { id: "linux-inventory", title: "Overview", Page: DoctorOverview };
export const servicesUi: ModuleUi = { id: `${AUDIT_MODULE_ID}:services`, title: "Services", Page: ServicesPage };
export const networkUi: ModuleUi = { id: `${AUDIT_MODULE_ID}:network`, title: "Network", Page: NetworkPage };
export const softwareUi: ModuleUi = { id: `${AUDIT_MODULE_ID}:software`, title: "Software", Page: SoftwarePage };

export const reportsUi: ModuleUi = { id: `${AUDIT_MODULE_ID}:reports`, title: "Reports", Page: ReportsPage };

export const diagnosticsUi: ModuleUi = {
  id: `${AUDIT_MODULE_ID}:diagnostics`,
  title: "Diagnostics",
  Page: ({ serverId, connected }) => <FindingsView serverId={serverId} connected={connected} noun="diagnostic" />,
};

export const securityUi: ModuleUi = {
  id: `${AUDIT_MODULE_ID}:security`,
  title: "Security",
  Page: ({ serverId, connected }) => (
    <FindingsView serverId={serverId} connected={connected} categories={SECURITY_CATEGORIES} noun="security" />
  ),
};
