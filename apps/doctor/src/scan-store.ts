// Per-server assessment state. Module level, so it survives page remounts when the user
// switches server or tab. Held in memory only: nothing is persisted.
//
// Rules this file enforces:
// - Steps run strictly one at a time, in the order `list_modules` reports for `linux-audit`.
// - Each scan has a generation number (`scanId`) per server. A response that belongs to an
//   older generation is dropped, so a late answer can never overwrite newer data.
// - Results are stored under the server id and the `host:port` they were collected from.
//   A different endpoint never shows them, and another server's results are never read.
// - Scan status (did every step run) is separate from coverage (how many checks were assessed).
import { useEffect, useState, useSyncExternalStore } from "react";
import { api, toIpcError, type Server } from "@ossiframe/client";
import { errorMessage } from "@ossiframe/ui";
import {
  AUDIT_MODULE_ID,
  type CheckResult,
  type ScanStatus,
  type StepResult,
  type Summary,
} from "./audit-types";

export type ScanRunState = "idle" | "running" | "complete" | "incomplete";

/** Why a scan ended before every step ran. `before` is the step that did not run. */
export interface StopReason {
  kind: "cancelled" | "connectionLost" | "failed";
  before: string | null;
  /** Short text for `failed`. */
  message: string | null;
}

export interface ScanEntry {
  /** Generation of the latest scan for this server. 0 before the first scan. */
  scanId: number;
  /** `host:port` the results were collected from. */
  endpointKey: string | null;
  state: ScanRunState;
  /** Results are kept but the connection is gone, so they may be out of date. */
  stale: boolean;
  staleAt: string | null;
  /** Step ids in scan order. */
  steps: string[];
  /** Step that is running now. */
  current: string | null;
  results: Record<string, StepResult>;
  /** Short message per step that failed (for example a timeout). */
  stepErrors: Record<string, string>;
  startedAt: string | null;
  finishedAt: string | null;
  stop: StopReason | null;
  /** Cancel was requested; no further step starts. */
  cancelling: boolean;
}

const EMPTY: ScanEntry = {
  scanId: 0,
  endpointKey: null,
  state: "idle",
  stale: false,
  staleAt: null,
  steps: [],
  current: null,
  results: {},
  stepErrors: {},
  startedAt: null,
  finishedAt: null,
  stop: null,
  cancelling: false,
};

let entries: ReadonlyMap<string, ScanEntry> = new Map();
/** Latest generation per server. Never deleted, so a removed and re-added id cannot reuse a number. */
const generations = new Map<string, number>();
const listeners = new Set<() => void>();

const now = () => new Date().toISOString();

function emit(next: Map<string, ScanEntry>) {
  entries = next;
  listeners.forEach((l) => l());
}

function put(serverId: string, entry: ScanEntry) {
  const next = new Map(entries);
  next.set(serverId, entry);
  emit(next);
}

/** Applies a patch only while `scanId` is still the latest generation for the server. */
function patch(serverId: string, scanId: number, p: Partial<ScanEntry> | ((e: ScanEntry) => Partial<ScanEntry>)): boolean {
  const cur = entries.get(serverId);
  if (!cur || generations.get(serverId) !== scanId) return false;
  put(serverId, { ...cur, ...(typeof p === "function" ? p(cur) : p) });
  return true;
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function endpointKey(server: Pick<Server, "host" | "port">): string {
  return `${server.host}:${server.port}`;
}

/**
 * Current entry for a server. Re-renders the component when it changes. When `endpoint` is
 * given and differs from the endpoint the results were collected from, the entry is empty.
 */
export function useScanEntry(serverId: string, endpoint?: string): ScanEntry {
  const entry = useSyncExternalStore(subscribe, () => entries.get(serverId) ?? EMPTY);
  if (endpoint !== undefined && entry.endpointKey !== null && entry.endpointKey !== endpoint) return EMPTY;
  return entry;
}

/**
 * `host:port` of the server's current profile, read from the server list. `null` until it is
 * known. Pages pass it to `useScanEntry` so results from another endpoint never show.
 * Re-reads when `connected` changes, which also happens after a profile edit and reconnect.
 */
export function useServerEndpoint(serverId: string, connected: boolean): string | null {
  // The value is tagged with the server it was read for, so a stale answer is never used for another server.
  const [read, setRead] = useState<{ serverId: string; endpoint: string | null } | null>(null);
  useEffect(() => {
    let live = true;
    api
      .listServers()
      .then((list) => {
        const s = list.find((x) => x.id === serverId);
        if (live) setRead({ serverId, endpoint: s ? endpointKey(s) : null });
      })
      .catch(() => {
        if (live) setRead({ serverId, endpoint: null });
      });
    return () => {
      live = false;
    };
  }, [serverId, connected]);
  return read?.serverId === serverId ? read.endpoint : null;
}

// ---- Derived values (pure) ----

export function allChecks(entry: ScanEntry): CheckResult[] {
  return entry.steps.flatMap((s) => entry.results[s]?.checks ?? []);
}

/** Counts by outcome and severity, and coverage. Mirrors `Summary::add` in `model.rs`. */
export function summarize(entry: ScanEntry): Summary {
  const s: Summary = {
    outcomes: { passed: 0, finding: 0, unavailable: 0, incomplete: 0, notApplicable: 0, error: 0 },
    severities: { critical: 0, high: 0, medium: 0, low: 0, info: 0 },
    coverage: { assessed: 0, notAssessed: 0, notApplicable: 0 },
  };
  for (const c of allChecks(entry)) {
    s.outcomes[c.outcome] += 1;
    if (c.outcome === "passed" || c.outcome === "finding") s.coverage.assessed += 1;
    else if (c.outcome === "notApplicable") s.coverage.notApplicable += 1;
    else s.coverage.notAssessed += 1;
    if (c.outcome === "finding" && c.severity) s.severities[c.severity] += 1;
  }
  return s;
}

/** Steps with no result, in scan order. */
export function missingSteps(entry: ScanEntry): string[] {
  return entry.steps.filter((s) => entry.results[s] === undefined);
}

/** `null` while idle or running. Says nothing about coverage. */
export function scanStatus(entry: ScanEntry): ScanStatus | null {
  return entry.state === "complete" || entry.state === "incomplete" ? entry.state : null;
}

/** Text for a scan that did not run every step; `null` when nothing is missing or still running. */
export function incompleteLabel(entry: ScanEntry): string | null {
  if (entry.state !== "incomplete") return null;
  const stop = entry.stop;
  const before = stop?.before ? ` before ${stop.before}` : "";
  if (stop?.kind === "cancelled") return `Incomplete: cancelled${before}`;
  if (stop?.kind === "connectionLost") return `Incomplete: connection lost${before}`;
  if (stop?.kind === "failed") return `Incomplete: ${stop.message ?? "scan failed"}`;
  const missing = missingSteps(entry);
  return missing.length > 0 ? `Incomplete: no result for ${missing.join(", ")}` : "Incomplete";
}

/** "Scan complete, 3 checks not assessed" style text. Never calls a scan clean. */
export function coverageLabel(entry: ScanEntry): string | null {
  const status = scanStatus(entry);
  if (status === null) return null;
  const n = summarize(entry).coverage.notAssessed;
  const head = status === "complete" ? "Scan complete" : "Scan incomplete";
  return n === 0 ? head : `${head}, ${n} ${n === 1 ? "check" : "checks"} not assessed`;
}

// ---- Actions ----

function finish(serverId: string, scanId: number, state: "complete" | "incomplete", stop: StopReason | null) {
  patch(serverId, scanId, { state, stop, current: null, finishedAt: now(), cancelling: false });
}

/**
 * Runs the assessment. Steps run one at a time. Nothing is retried. Safe to call while a scan
 * is running (ignored). Starting a scan clears the previous results.
 */
export async function startScan(serverId: string): Promise<void> {
  if (entries.get(serverId)?.state === "running") return;
  const scanId = (generations.get(serverId) ?? 0) + 1;
  generations.set(serverId, scanId);
  put(serverId, { ...EMPTY, scanId, state: "running", startedAt: now() });
  const live = () => generations.get(serverId) === scanId;

  let steps: string[];
  try {
    const server = (await api.listServers()).find((s) => s.id === serverId);
    if (!live()) return;
    if (!server || server.state !== "connected") {
      finish(serverId, scanId, "incomplete", { kind: "connectionLost", before: null, message: null });
      return;
    }
    const module = (await api.listModules()).find((m) => m.id === AUDIT_MODULE_ID);
    if (!live()) return;
    if (!module) {
      finish(serverId, scanId, "incomplete", { kind: "failed", before: null, message: "the audit module is not available" });
      return;
    }
    steps = module.operations.map((o) => o.id);
    patch(serverId, scanId, { endpointKey: endpointKey(server), steps });
  } catch (e) {
    if (live()) finish(serverId, scanId, "incomplete", { kind: "failed", before: null, message: errorMessage(toIpcError(e)) });
    return;
  }

  for (const step of steps) {
    if (!live()) return;
    if (entries.get(serverId)?.cancelling) {
      finish(serverId, scanId, "incomplete", { kind: "cancelled", before: step, message: null });
      return;
    }
    patch(serverId, scanId, { current: step });
    try {
      const result = await api.runOperation<StepResult>(serverId, AUDIT_MODULE_ID, step, null);
      if (!live()) return;
      if (result.step !== step) {
        patch(serverId, scanId, (e) => ({ stepErrors: { ...e.stepErrors, [step]: "Unexpected result." } }));
      } else {
        patch(serverId, scanId, (e) => ({ results: { ...e.results, [step]: result } }));
      }
    } catch (e) {
      if (!live()) return;
      const err = toIpcError(e);
      if (err.kind === "cancelled") {
        finish(serverId, scanId, "incomplete", { kind: "cancelled", before: step, message: null });
        return;
      }
      if (err.kind === "connectionLost" || err.kind === "notConnected") {
        patch(serverId, scanId, { stale: true, staleAt: now() });
        finish(serverId, scanId, "incomplete", { kind: "connectionLost", before: step, message: null });
        return;
      }
      // A timeout or other failure of one step is recorded; the scan goes on without a retry.
      patch(serverId, scanId, (cur) => ({ stepErrors: { ...cur.stepErrors, [step]: errorMessage(err) } }));
    }
  }

  const done = entries.get(serverId);
  if (!done || !live()) return;
  finish(serverId, scanId, missingSteps(done).length === 0 ? "complete" : "incomplete", null);
}

/** Stops the scan: cancels the running command and starts no further step. Results are kept. */
export async function cancelScan(serverId: string): Promise<void> {
  const cur = entries.get(serverId);
  if (!cur || cur.state !== "running") return;
  put(serverId, { ...cur, cancelling: true });
  try {
    await api.cancelOperation(serverId);
  } catch {
    // The loop still stops before the next step. The running command ends on its own.
  }
}

/**
 * The connection is gone or the profile changed while disconnected. Results stay, marked
 * stale; a running scan ends as incomplete and its late responses are dropped.
 */
export function markStale(serverId: string) {
  const cur = entries.get(serverId);
  if (cur && cur.scanId > 0) {
    if (cur.state === "running") {
      generations.set(serverId, cur.scanId + 1);
      put(serverId, {
        ...cur,
        scanId: cur.scanId + 1,
        state: "incomplete",
        stale: true,
        staleAt: now(),
        stop: { kind: "connectionLost", before: cur.current, message: null },
        current: null,
        cancelling: false,
        finishedAt: now(),
      });
    } else if (!cur.stale) {
      put(serverId, { ...cur, stale: true, staleAt: now() });
    }
  }
  void reconcileEndpoint(serverId);
}

/** Drops the results when the profile's host or port no longer matches where they came from. */
export async function reconcileEndpoint(serverId: string): Promise<void> {
  const cur = entries.get(serverId);
  if (!cur?.endpointKey) return;
  try {
    const server = (await api.listServers()).find((s) => s.id === serverId);
    const latest = entries.get(serverId);
    if (server && latest?.endpointKey && latest.endpointKey !== endpointKey(server)) clearServer(serverId);
  } catch {
    // Keep the entry; pages that know the endpoint also filter by it.
  }
}

/** Forget everything about a server (profile removed). Invalidates any running scan. */
export function clearServer(serverId: string) {
  generations.set(serverId, (generations.get(serverId) ?? 0) + 1);
  const next = new Map(entries);
  next.delete(serverId);
  emit(next);
}
