// Hand-written mirrors of the `linux-audit` Rust results (modules/linux-audit/src/model.rs
// and the step evidence in steps.rs and certs.rs). All fields are camelCase on the wire.
// Keep in sync with the Rust definitions. Remote strings are data: render them as text only.
import type { DiskInfo, ListeningSocket, Package, Service } from "@ossiframe/client";

/** The module id and the step ids in scan order come from `list_modules`; this is only the module id. */
export const AUDIT_MODULE_ID = "linux-audit";

export type Outcome = "passed" | "finding" | "unavailable" | "incomplete" | "notApplicable" | "error";
export type Severity = "critical" | "high" | "medium" | "low" | "info";
export type Category = "resources" | "services" | "network" | "security" | "updates";

export interface CheckResult {
  ruleId: string;
  title: string;
  category: Category;
  outcome: Outcome;
  /** Present only when `outcome` is `finding`. The key is absent otherwise. */
  severity?: Severity;
  resource: string | null;
  evidence: string[];
  explanation: string;
  /** Advice text only. Never executed. */
  recommendation: string | null;
  observedAt: string;
  limitations: string[];
  /** True when the conclusion goes beyond observed facts. */
  inferred: boolean;
}

/** Evidence of the `resources` step. */
export interface ResourcesEvidence {
  hostname: string | null;
  os: { prettyName: string | null; id: string | null; versionId: string | null } | null;
  kernel: string | null;
  memoryTotalKb: number | null;
  memoryAvailableKb: number | null;
  swapTotalKb: number | null;
  swapFreeKb: number | null;
  filesystems: DiskInfo[];
  filesystemsTruncated: boolean;
}

/** Evidence of the `services` step: counts and the merged, bounded unit list. */
export interface ServicesEvidence {
  available: boolean;
  truncated: boolean;
  unitCount: number;
  failedCount: number;
  services: Service[];
  limitations: string[];
}

/** Evidence of the `network` step. */
export interface NetworkEvidence {
  available: boolean;
  truncated: boolean;
  socketCount: number;
  databasePortSockets: ListeningSocket[];
  /** The full bounded socket list. */
  sockets: ListeningSocket[];
  limitations: string[];
}

/** Evidence of the `software` step. */
export interface SoftwareEvidence {
  available: boolean;
  truncated: boolean;
  packages: Package[];
  limitations: string[];
}

/** Evidence of the `ssh` step. Values are the first value seen, or null when not set. */
export interface SshEvidence {
  files: string[];
  values: {
    passwordAuthentication: string | null;
    permitRootLogin: string | null;
  };
  matchBlocks: boolean;
  readInFull: boolean;
  limitations: string[];
}

/** Evidence of the `firewall` step. The Rust side serializes an Option, so it may be null. */
export interface FirewallEvidence {
  ufwConfEnabled: boolean | null;
}

/** Evidence of the `updates` step. Cached data, not live repository state. */
export interface UpdatesEvidence {
  pendingUpdates: number | null;
  pendingSecurity: number | null;
  metadataAgeSeconds: number | null;
  rebootRequired: boolean | null;
  source: string;
}

export interface CertEvidence {
  path: string;
  configFile: string;
  /** Negative when expired, null when it could not be read. */
  secondsUntilExpiry: number | null;
}

/** Evidence of the `certificates` step. */
export interface CertificatesEvidence {
  certs: CertEvidence[];
  /** Names of the nginx config files that were read. */
  filesRead: string[];
  configFound: boolean;
  limitations: string[];
}

export interface EvidenceByStep {
  resources: ResourcesEvidence;
  services: ServicesEvidence;
  network: NetworkEvidence;
  software: SoftwareEvidence;
  ssh: SshEvidence;
  firewall: FirewallEvidence;
  updates: UpdatesEvidence;
  certificates: CertificatesEvidence;
}

export type StepId = keyof EvidenceByStep;

/** One step result. `E` is the evidence type of that step; `unknown` when the step id is not known. */
export interface StepResult<E = unknown> {
  step: string;
  observedAt: string;
  evidence: E;
  checks: CheckResult[];
}

export interface OutcomeCounts {
  passed: number;
  finding: number;
  unavailable: number;
  incomplete: number;
  notApplicable: number;
  error: number;
}

export interface SeverityCounts {
  critical: number;
  high: number;
  medium: number;
  low: number;
  info: number;
}

/** How much was assessed. Separate from the scan status. */
export interface Coverage {
  /** `passed` + `finding`. */
  assessed: number;
  /** `unavailable` + `incomplete` + `error`. */
  notAssessed: number;
  notApplicable: number;
}

export interface Summary {
  outcomes: OutcomeCounts;
  severities: SeverityCounts;
  coverage: Coverage;
}

/** Whether every step ran to the end. Says nothing about coverage. */
export type ScanStatus = "complete" | "incomplete";
