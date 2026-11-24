// Per-server cache of the last `server-info` result. Module level, so it survives
// the page remounting when the user switches server or tab. Held in memory only.
import { useSyncExternalStore } from "react";
import { api, toIpcError, type ServerInfo } from "@ossiframe/client";
import { errorMessage } from "@ossiframe/ui";

export interface InfoState {
  info: ServerInfo;
  stale: boolean;
}

export interface ServerEntry {
  info: InfoState | undefined;
  loading: boolean;
  error: string | null;
}

const EMPTY: ServerEntry = { info: undefined, loading: false, error: null };

let entries: ReadonlyMap<string, ServerEntry> = new Map();
const listeners = new Set<() => void>();

function update(id: string, patch: Partial<ServerEntry>) {
  const next = new Map(entries);
  next.set(id, { ...(entries.get(id) ?? EMPTY), ...patch });
  entries = next;
  listeners.forEach((l) => l());
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Current entry for a server. Re-renders the component when it changes. */
export function useServerEntry(serverId: string): ServerEntry {
  return useSyncExternalStore(subscribe, () => entries.get(serverId) ?? EMPTY);
}

/** The cached data may be out of date (disconnected, connection lost, profile edited). */
export function markStale(serverId: string) {
  const cur = entries.get(serverId)?.info;
  if (cur && !cur.stale) update(serverId, { info: { info: cur.info, stale: true } });
}

/** Forget everything about a server (profile removed). */
export function clearServer(serverId: string) {
  const next = new Map(entries);
  next.delete(serverId);
  entries = next;
  listeners.forEach((l) => l());
}

/** Reads the inventory through the typed `linux-inventory` / `server-info` operation. */
export async function loadServerInfo(serverId: string) {
  update(serverId, { loading: true, error: null });
  try {
    const info = await api.runOperation<ServerInfo>(serverId, "linux-inventory", "server-info", null);
    update(serverId, { info: { info, stale: false }, loading: false });
  } catch (e) {
    const err = toIpcError(e);
    if (err.kind === "connectionLost" || err.kind === "notConnected") markStale(serverId);
    update(serverId, { loading: false, error: errorMessage(err) });
  }
}
