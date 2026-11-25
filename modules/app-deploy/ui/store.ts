// Which screen the Applications tab shows for each server. Module level, so the
// screen (for example a running operation) survives switching tabs or servers.
// Held in memory only. Plans and operations live in the backend.
import { useSyncExternalStore } from "react";

export type View =
  | { kind: "list" }
  | { kind: "create" }
  | { kind: "detail"; appId: string }
  /** `slug` lets the finished screen find an app that was just created. */
  | { kind: "operation"; operationId: string; slug: string; appId: string | null; returnToList?: boolean };

const LIST: View = { kind: "list" };
let views: ReadonlyMap<string, View> = new Map();
const listeners = new Set<() => void>();

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function useView(serverId: string): View {
  return useSyncExternalStore(subscribe, () => views.get(serverId) ?? LIST);
}

export function setView(serverId: string, view: View) {
  const next = new Map(views);
  next.set(serverId, view);
  views = next;
  listeners.forEach((l) => l());
}

/** Forget everything about a server (profile removed). */
export function clearServer(serverId: string) {
  const next = new Map(views);
  next.delete(serverId);
  views = next;
  listeners.forEach((l) => l());
}
