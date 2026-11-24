// Typed wrappers for the Ossiframe desktop IPC commands. This is the only
// module that calls `invoke`.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { ConnState, ConnectOutcome, IpcError, ProfileInput, Server, ServerInfo } from "./types";

export * from "./types";

const KINDS = new Set([
  "validation", "notFound", "unreachable", "connectTimeout", "authTimeout", "hostKeyMismatch", "authFailed",
  "credentialRequired", "keyFileUnreadable", "notConnected", "commandTimeout", "cancelled",
  "connectionLost", "keychain", "storage", "hostKeyApproval", "alreadyPinned",
  "confirmationMismatch", "protocol", "internal",
]);

export function isIpcError(e: unknown): e is IpcError {
  return typeof e === "object" && e !== null && "kind" in e && KINDS.has(String((e as { kind: unknown }).kind));
}

/** Normalizes anything thrown by `invoke` into an `IpcError`. */
export function toIpcError(e: unknown): IpcError {
  return isIpcError(e) ? e : { kind: "internal" };
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw toIpcError(e);
  }
}

export const api = {
  listServers: () => call<Server[]>("list_servers"),
  addServer: (input: ProfileInput) => call<Server>("add_server", { input }),
  updateServer: (serverId: string, input: ProfileInput) => call<Server>("update_server", { serverId, input }),
  removeServer: (serverId: string) => call<void>("remove_server", { serverId }),
  /** `credential` is used for this attempt only unless `saveSecret` is true. */
  connectServer: (serverId: string, credential: string | null, saveSecret: boolean) =>
    call<ConnectOutcome>("connect_server", { serverId, credential, saveSecret }),
  disconnectServer: (serverId: string) => call<void>("disconnect_server", { serverId }),
  trustHostKey: (serverId: string, token: string) => call<void>("trust_host_key", { serverId, token }),
  replaceHostKey: (serverId: string, token: string, confirmHost: string) =>
    call<void>("replace_host_key", { serverId, token, confirmHost }),
  getServerInfo: (serverId: string) => call<ServerInfo>("get_server_info", { serverId }),
  cancelOperation: (serverId: string) => call<void>("cancel_operation", { serverId }),
};

export interface ConnectionStateEvent {
  serverId: string;
  state: ConnState;
}

export function onConnectionState(cb: (e: ConnectionStateEvent) => void): Promise<UnlistenFn> {
  return listen<ConnectionStateEvent>("connection-state", (e) => cb(e.payload));
}

export function onConnectionLost(cb: (serverId: string) => void): Promise<UnlistenFn> {
  return listen<{ serverId: string }>("connection-lost", (e) => cb(e.payload.serverId));
}
