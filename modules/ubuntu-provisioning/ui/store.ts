// Per-server provisioning state. Module level, so it survives the pages
// remounting when the user switches tab or server. Held in memory only.
// The backend owns plans and operations; this store only holds what the backend
// returned and the ids/digest needed to approve it. Nothing here is sent back
// except ids and the digest.
import { useSyncExternalStore } from "react";
import {
  onOperationUpdated,
  provisioningApi,
  toIpcError,
  type ComponentInfo,
  type IpcError,
  type OperationDetailDto,
  type OperationState,
  type OperationSummaryDto,
  type PlanDto,
  type PreflightDto,
} from "@ossiframe/client";
import { errorMessage } from "@ossiframe/ui";

export interface StoredPlan {
  dto: PlanDto;
  /** `Date.now()` when the plan was received. The countdown counts from here. */
  receivedAt: number;
  /** Selection the plan was created for. */
  selection: string[];
}

export interface ServerEntry {
  selection: string[] | null;
  preflight: PreflightDto | null;
  plan: StoredPlan | null;
  /** Operation shown on the Operation page. */
  operationId: string | null;
  operation: OperationDetailDto | null;
  history: OperationSummaryDto[];
  busy: "preflight" | "plan" | "approve" | "history" | null;
  error: IpcError | null;
}

const EMPTY: ServerEntry = {
  selection: null,
  preflight: null,
  plan: null,
  operationId: null,
  operation: null,
  history: [],
  busy: null,
  error: null,
};

let entries: ReadonlyMap<string, ServerEntry> = new Map();
const listeners = new Set<() => void>();

function emit() {
  listeners.forEach((l) => l());
}

function update(id: string, patch: Partial<ServerEntry>) {
  const next = new Map(entries);
  next.set(id, { ...(entries.get(id) ?? EMPTY), ...patch });
  entries = next;
  emit();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  void ensureEventListener();
  return () => {
    listeners.delete(listener);
  };
}

export function useServerEntry(serverId: string): ServerEntry {
  return useSyncExternalStore(subscribe, () => entries.get(serverId) ?? EMPTY);
}

interface ComponentsState {
  list: ComponentInfo[] | null;
  error: string | null;
}
let componentsSnapshot: ComponentsState = { list: null, error: null };

export function useComponents(): ComponentsState {
  return useSyncExternalStore(subscribe, () => componentsSnapshot);
}

function setComponents(list: ComponentInfo[] | null, error: string | null) {
  componentsSnapshot = { list, error };
  emit();
}

export async function loadComponents() {
  try {
    setComponents(await provisioningApi.components(), null);
  } catch (e) {
    setComponents(null, errorMessage(toIpcError(e)));
  }
}

/** The selection in effect: the user's choice, else every non-optional component. */
export function effectiveSelection(entry: ServerEntry, list: ComponentInfo[]): string[] {
  return entry.selection ?? list.filter((c) => !c.optional).map((c) => c.id);
}

export function setSelection(serverId: string, selection: string[]) {
  // A different selection makes a stored plan and preflight stale for review.
  update(serverId, { selection, plan: null });
}

// ---- preflight and plan ---------------------------------------------------------

export async function runPreflight(serverId: string, selection: string[]) {
  update(serverId, { busy: "preflight", error: null });
  try {
    const preflight = await provisioningApi.preflight(serverId, selection);
    update(serverId, { preflight, busy: null });
  } catch (e) {
    update(serverId, { busy: null, error: toIpcError(e) });
  }
}

export async function createPlan(serverId: string, selection: string[]) {
  update(serverId, { busy: "plan", error: null });
  try {
    const dto = await provisioningApi.createPlan(serverId, selection);
    update(serverId, { plan: { dto, receivedAt: Date.now(), selection }, busy: null });
  } catch (e) {
    update(serverId, { busy: null, plan: null, error: toIpcError(e) });
  }
}

/** Sends only the server id, the plan id and the digest the backend returned. */
export async function approvePlan(serverId: string): Promise<boolean> {
  const plan = entries.get(serverId)?.plan;
  if (!plan) return false;
  update(serverId, { busy: "approve", error: null });
  try {
    const started = await provisioningApi.approvePlan(serverId, plan.dto.planId, plan.dto.digest);
    update(serverId, { busy: null, plan: null, operationId: started.operationId, operation: null });
    await refreshOperation(started.operationId);
    void loadHistory(serverId);
    return true;
  } catch (e) {
    // The plan is single use and the backend may have dropped it, so it is kept
    // on screen only when the failure says it is still reviewable.
    update(serverId, { busy: null, error: toIpcError(e) });
    return false;
  }
}

export function discardPlan(serverId: string) {
  update(serverId, { plan: null, error: null });
}

export function clearError(serverId: string) {
  update(serverId, { error: null });
}

// ---- operations -----------------------------------------------------------------

export function isTerminal(state: OperationState): boolean {
  return state === "succeeded" || state === "failed" || state === "cancelled";
}

function serverOf(operationId: string): string | null {
  for (const [id, e] of entries) if (e.operationId === operationId) return id;
  return null;
}

export async function refreshOperation(operationId: string) {
  const serverId = serverOf(operationId);
  if (!serverId) return;
  try {
    const operation = await provisioningApi.getOperation(operationId);
    if (entries.get(serverId)?.operationId === operationId) update(serverId, { operation });
  } catch (e) {
    update(serverId, { error: toIpcError(e) });
  }
}

export function showOperation(serverId: string, operationId: string) {
  update(serverId, { operationId, operation: null, error: null });
  void refreshOperation(operationId);
}

export async function loadHistory(serverId: string) {
  try {
    const history = await provisioningApi.listOperations(serverId, 50);
    update(serverId, { history });
  } catch (e) {
    update(serverId, { error: toIpcError(e) });
  }
}

/** Shows the newest operation when the page has none selected. */
export async function ensureOperation(serverId: string) {
  const cur = entries.get(serverId);
  if (cur?.operationId) {
    await refreshOperation(cur.operationId);
    return;
  }
  await loadHistory(serverId);
  const newest = entries.get(serverId)?.history[0];
  if (newest) showOperation(serverId, newest.id);
}

/** Cancel is a request. The backend finishes the current step first. */
export async function cancelOperation(serverId: string, operationId: string) {
  try {
    await provisioningApi.cancel(operationId);
    await refreshOperation(operationId);
  } catch (e) {
    update(serverId, { error: toIpcError(e) });
  }
}

export async function releaseStaleLock(serverId: string): Promise<boolean> {
  try {
    await provisioningApi.releaseStaleLock(serverId);
    update(serverId, { error: null });
    return true;
  } catch (e) {
    update(serverId, { error: toIpcError(e) });
    return false;
  }
}

export function setError(serverId: string, e: unknown) {
  update(serverId, { error: toIpcError(e) });
}

/** Forget everything about a server (profile removed). */
export function clearServer(serverId: string) {
  const next = new Map(entries);
  next.delete(serverId);
  entries = next;
  emit();
}

/** The connection ended or the profile changed: a held plan is no longer reviewable. */
export function markStale(serverId: string) {
  const cur = entries.get(serverId);
  if (cur && (cur.plan || cur.preflight)) update(serverId, { plan: null, preflight: null });
}

// ---- operation-updated event -----------------------------------------------------

let eventListener: Promise<void> | null = null;

function ensureEventListener(): Promise<void> {
  eventListener ??= onOperationUpdated((e) => {
    const cur = entries.get(e.serverId);
    if (cur?.operationId === e.operationId) void refreshOperation(e.operationId);
    if (cur) void loadHistory(e.serverId);
  }).then(
    () => undefined,
    () => {
      // No Tauri runtime (for example a plain browser). Polling still works.
      eventListener = null;
    },
  );
  return eventListener;
}

export function describeError(e: IpcError | null): string | null {
  return e ? errorMessage(e) : null;
}
