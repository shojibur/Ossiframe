// Typed wrappers for the Ossiframe desktop IPC commands. With the connection
// hook, this is the only package that calls `invoke` and `listen`.

import { call } from "./call";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { ConnState, ConnectOutcome, ModuleInfo, ProfileInput, Server } from "./types";

export * from "./types";

export { isIpcError, toIpcError } from "./call";

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
  /** Runs one operation of a module compiled into the app. `args` is validated by the module. */
  runOperation: <T = unknown>(
    serverId: string,
    moduleId: string,
    operationId: string,
    args: unknown = null,
  ) => call<T>("run_operation", { serverId, moduleId, operationId, args }),
  listModules: () => call<ModuleInfo[]>("list_modules"),
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
