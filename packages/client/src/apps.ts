// Typed wrappers for the `app_*` IPC commands (Deploy only). Mirrors
// apps/deploy/src-tauri/src/apps/{commands,dto}.rs and the module types in
// ossiframe-mod-app-deploy. Keep in sync with the Rust definitions.
//
// No type here holds a secret: a secret environment value never reaches the
// frontend, and the database password is never returned.

import { call } from "./call";
import type { OperationStartedDto, PlanDto } from "./provisioning";

export type AppTypeId = "laravel" | "php" | "static";
export type DatabaseUse = "default" | "optIn" | "never";

export interface AppTypeInfo {
  appType: AppTypeId;
  id: string;
  title: string;
  description: string;
  usesPhp: boolean;
  database: DatabaseUse;
  entryFile: string;
  entryFileFixed: boolean;
}

export interface AppSummaryDto {
  id: string;
  slug: string;
  appType: string;
  domain: string;
  /** Application state as stored, for example `active` or `awaitingDeploy`. */
  state: string;
  currentReleaseId: string | null;
}

export interface ReleaseDto {
  /** UTC timestamp id, as used by `rollbackPlan`. */
  id: string;
  commitSha: string | null;
  branch: string;
  status: string;
  operationId: string | null;
  createdAt: string;
}

export interface AppDto {
  id: string;
  serverId: string;
  slug: string;
  appType: string;
  domain: string;
  repoUrl: string;
  repoBranch: string;
  entryFile: string;
  state: string;
  hasDatabase: boolean;
  needsDeployKey: boolean;
  currentRelease: ReleaseDto | null;
  createdAt: string;
  updatedAt: string;
}

export interface CreateAppInput {
  serverId: string;
  appType: string;
  slug: string;
  domain: string;
  repoUrl: string;
  repoBranch: string;
  entryFile?: string | null;
  withDatabase: boolean;
  disableDefaultSite: boolean;
  /** `SHA256:...` fingerprints the user confirmed for the Git host. */
  trustedGitHostFingerprints?: string[] | null;
}

/** One changed variable. Send only changed keys; omit unchanged secrets. */
export interface EnvChange {
  key: string;
  /** New value. Leave out when `delete` is true. */
  value?: string | null;
  delete?: boolean;
}

/** A secret entry has `value: null`; `isSet` tells whether it has a value. */
export interface EnvEntryDto {
  key: string;
  value?: string | null;
  secret: boolean;
  isSet: boolean;
}

export type AccessResult =
  | "ok"
  | "authDenied"
  | "hostKeyFailed"
  | "branchMissing"
  | "repoNotFound"
  | "privateHttps"
  | "unreachable"
  | "failed";

export interface AccessCheckDto {
  result: AccessResult;
  passed: boolean;
  message: string;
}

export interface DeployKeyStatus {
  host: string;
  /** The public half of the deploy key. Safe to show and copy. */
  publicKey: string;
  instructions: string;
  access: AccessResult;
  accessMessage: string;
}

export const appsApi = {
  types: () => call<AppTypeInfo[]>("app_types"),
  list: (serverId: string) => call<AppSummaryDto[]>("app_list", { serverId }),
  get: (appId: string) => call<AppDto>("app_get", { appId }),
  createPlan: (i: CreateAppInput) =>
    call<PlanDto>("app_create_plan", {
      serverId: i.serverId,
      appType: i.appType,
      slug: i.slug,
      domain: i.domain,
      repoUrl: i.repoUrl,
      repoBranch: i.repoBranch,
      entryFile: i.entryFile ?? null,
      withDatabase: i.withDatabase,
      disableDefaultSite: i.disableDefaultSite,
      trustedGitHostFingerprints: i.trustedGitHostFingerprints ?? null,
    }),
  redeployPlan: (appId: string, repoBranch: string | null, runMigrations: boolean) =>
    call<PlanDto>("app_redeploy_plan", { appId, repoBranch, runMigrations }),
  rollbackPlan: (appId: string, releaseId: string) =>
    call<PlanDto>("app_rollback_plan", { appId, releaseId }),
  removePlan: (appId: string, dropDatabase: boolean, deleteFiles: boolean) =>
    call<PlanDto>("app_remove_plan", { appId, dropDatabase, deleteFiles }),
  /** Approves the stored plan. Only ids and the digest are sent, never plan content. */
  approvePlan: (serverId: string, planId: string, digest: string) =>
    call<OperationStartedDto>("app_approve_plan", { serverId, planId, digest }),
  getEnv: (appId: string) => call<EnvEntryDto[]>("app_get_env", { appId }),
  /** Returns a plan to approve; nothing is written until it is approved. */
  setEnv: (appId: string, changes: EnvChange[]) => call<PlanDto>("app_set_env", { appId, changes }),
  listReleases: (appId: string) => call<ReleaseDto[]>("app_list_releases", { appId }),
  deployKeyStatus: (appId: string) => call<DeployKeyStatus>("app_deploy_key_status", { appId }),
  checkAccess: (appId: string) => call<AccessCheckDto>("app_check_access", { appId }),
};
