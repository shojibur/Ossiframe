// Shared `invoke` wrapper and IpcError normalization. Not exported from the
// package index except `isIpcError` and `toIpcError` (through `api.ts`).

import { invoke } from "@tauri-apps/api/core";
import type { IpcError } from "./types";

const KINDS = new Set([
  "validation", "notFound", "unreachable", "connectTimeout", "authTimeout", "hostKeyMismatch", "authFailed",
  "credentialRequired", "keyFileUnreadable", "notConnected", "commandTimeout", "cancelled",
  "connectionLost", "keychain", "storage", "hostKeyApproval", "alreadyPinned",
  "confirmationMismatch", "protocol", "operationFailed", "plan", "internal",
]);

export function isIpcError(e: unknown): e is IpcError {
  return typeof e === "object" && e !== null && "kind" in e && KINDS.has(String((e as { kind: unknown }).kind));
}

/** Normalizes anything thrown by `invoke` into an `IpcError`. */
export function toIpcError(e: unknown): IpcError {
  return isIpcError(e) ? e : { kind: "internal" };
}

export async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw toIpcError(e);
  }
}
