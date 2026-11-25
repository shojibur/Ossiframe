// Typed wrappers for the `provision_*` IPC commands and the `operation-updated`
// event. Mirrors ossiframe-desktop::provisioning::commands and the serde shapes
// in ossiframe-core::provision. Keep in sync with the Rust definitions.
//
// Exported separately from `api` on purpose: read-only apps (Doctor) do not
// register these commands, so they must not call them by accident.

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { call } from "./call";

// ---- ossiframe-core::provision ----------------------------------------------

export type OperationState =
  | "pending"
  | "running"
  | "verifying"
  | "succeeded"
  | "failed"
  | "cancelled"
  | "unknownOutcome";

export type ServerClass = "fresh" | "existingUnmanaged" | "ossiframeManaged" | "partial";
export type SudoMode = "passwordless" | "passwordRequired" | "notPermitted" | "unavailable";
export type Severity = "blocker" | "warning";
export type StepMode = "foreground" | "durable" | "gate";
export type Privilege = "user" | "root";
export type CheckStatus = "pass" | "fail" | "warn" | "skipped";

/** Rust `Duration` as serialized by serde. */
export interface DurationDto {
  secs: number;
  nanos: number;
}

export interface PlanTarget {
  serverId: string;
  name: string;
  host: string;
  port: number;
  user: string;
  hostKeySha256: string;
}

export interface PackageChange {
  name: string;
  installedVersion: string | null;
}

export interface FileChange {
  path: string;
  description: string;
}

export interface ServiceEffect {
  unit: string;
  effect: string;
}

export interface Resources {
  diskMbNeeded: number;
  memMbMin: number;
}

export interface CommandSpec {
  program: string;
  args: string[];
  timeout: DurationDto;
  privilege: Privilege;
}

export interface PlanStep {
  id: string;
  title: string;
  command: CommandSpec;
  mode: StepMode;
  deadline: DurationDto;
}

export type PackageState =
  | { state: "absent" }
  | { state: "installed"; version: string }
  | { state: "other"; status: string };

export interface Preconditions {
  osVersion: string;
  packages: Record<string, PackageState>;
  sudo: SudoMode;
  packageSources: string[];
  watchedListings: Record<string, string[]>;
  minMemMb: number;
  minDiskMb: number;
  serverManaged: boolean;
}

export interface Plan {
  target: PlanTarget;
  os: string;
  components: string[];
  alreadyPresent: string[];
  packages: PackageChange[];
  files: FileChange[];
  services: ServiceEffect[];
  resources: Resources;
  privileges: string[];
  limitations: string[];
  steps: PlanStep[];
  preconditions: Preconditions;
}

export interface Conflict {
  id: string;
  severity: Severity;
  message: string;
}

export interface PreflightReport {
  /** RFC 3339. */
  observedAt: string;
  os: string;
  sudo: SudoMode;
  serverClass: ServerClass;
  packages: Record<string, PackageState>;
  totalMemMb: number | null;
  freeDiskMb: number | null;
  packageSources: string[];
  sites: string[];
  /** Candidate version per component package, from the server's current APT lists. Information only. */
  candidateVersions: Record<string, string>;
  conflicts: Conflict[];
}

export interface CheckResult {
  component: string;
  id: string;
  title: string;
  essential: boolean;
  status: CheckStatus;
  detail: string;
}

export interface ComponentVerdict {
  component: string;
  verified: boolean;
}

export interface ComponentInfo {
  id: string;
  title: string;
  description: string;
  requires: string[];
  /** Packages that make up the component. */
  packages: string[];
  optional: boolean;
}

// ---- command DTOs --------------------------------------------------------------

export interface PreflightDto {
  report: PreflightReport;
  canProvision: boolean;
}

export interface PlanDto {
  planId: string;
  digest: string;
  expiresInSecs: number;
  plan: Plan;
  /** Preflight conflicts that did not block planning. */
  warnings: Conflict[];
}

export interface OperationStartedDto {
  operationId: string;
}

export interface EvidenceItem {
  label: string;
  value: string;
}

export interface EvidenceReport {
  items: EvidenceItem[];
}

export interface ReconcileDto {
  state: OperationState;
  summary: string;
  evidence: EvidenceReport;
}

export interface ResolveDto {
  evidence: EvidenceReport;
}

export interface SnapshotDto {
  name: string;
  host: string;
  port: number;
  user: string;
  hostKeySha256: string;
}

export interface OperationSummaryDto {
  id: string;
  /** `null` after the server profile was removed. */
  serverId: string | null;
  server: SnapshotDto;
  kind: string;
  planDigest: string;
  state: OperationState;
  cancelRequested: boolean;
  createdAt: string;
  startedAt: string | null;
  finishedAt: string | null;
  error: string | null;
}

export interface StepDto {
  idx: number;
  stepId: string;
  title: string;
  unitName: string | null;
  status: string;
  startedAt: string | null;
  finishedAt: string | null;
  exitCode: number | null;
  /** Redacted, bounded plain text. Render it as text. */
  output: string | null;
}

export interface EventDto {
  kind: string;
  fromState: OperationState | null;
  toState: OperationState | null;
  unitName: string | null;
  detail: string | null;
  at: string;
}

/** Stored verification document. */
export interface VerificationDoc {
  checks: CheckResult[];
  verdicts: ComponentVerdict[];
}

/** Stored when the user closed an UnknownOutcome operation. */
export interface ResolutionDoc {
  resolution: "markedResolved";
  decidedAt: string;
  decision: string;
  evidence: EvidenceReport;
}

export interface OperationDetailDto extends OperationSummaryDto {
  plan: Plan | null;
  steps: StepDto[];
  verification: VerificationDoc | null;
  resolution: ResolutionDoc | null;
  events: EventDto[];
}

// ---- commands -------------------------------------------------------------------

export const provisioningApi = {
  components: () => call<ComponentInfo[]>("provision_components"),
  preflight: (serverId: string, selection: string[]) =>
    call<PreflightDto>("provision_preflight", { serverId, selection }),
  createPlan: (serverId: string, selection: string[]) =>
    call<PlanDto>("provision_create_plan", { serverId, selection }),
  /** Approves the stored plan. Only ids and the digest are sent, never plan content. */
  approvePlan: (serverId: string, planId: string, digest: string) =>
    call<OperationStartedDto>("provision_approve_plan", { serverId, planId, digest }),
  cancel: (operationId: string) => call<void>("provision_cancel", { operationId }),
  reconcile: (operationId: string) => call<ReconcileDto>("provision_reconcile", { operationId }),
  markResolved: (operationId: string) => call<ResolveDto>("provision_mark_resolved", { operationId }),
  releaseStaleLock: (serverId: string) => call<void>("provision_release_stale_lock", { serverId }),
  getOperation: (operationId: string) => call<OperationDetailDto>("provision_get_operation", { operationId }),
  /** The backend clamps `limit` to 1..50 (default 20). */
  listOperations: (serverId?: string | null, limit?: number | null) =>
    call<OperationSummaryDto[]>("provision_list_operations", { serverId: serverId ?? null, limit: limit ?? null }),
};

// ---- event ----------------------------------------------------------------------

export interface OperationUpdatedEvent {
  serverId: string;
  operationId: string;
  state: OperationState;
}

/** Fires on every state change. Read the operation with `provisioningApi.getOperation`. */
export function onOperationUpdated(cb: (e: OperationUpdatedEvent) => void): Promise<UnlistenFn> {
  return listen<OperationUpdatedEvent>("operation-updated", (e) => cb(e.payload));
}
