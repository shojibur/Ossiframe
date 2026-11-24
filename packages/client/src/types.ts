// Hand-written mirrors of the Rust DTOs in ossiframe-desktop::ipc and the
// Doctor inventory. Keep in sync with the Rust definitions.

export type AuthKind = "keyFile" | "password";
export type ConnState = "disconnected" | "connecting" | "connected" | "error";

export interface HostKey {
  keyType: string;
  sha256: string;
}

export interface Server {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  authKind: AuthKind;
  keyPath: string | null;
  hasSavedSecret: boolean;
  pinnedHostKey: HostKey | null;
  state: ConnState;
}

export interface ProfileInput {
  name: string;
  host: string;
  port: number;
  username: string;
  authKind: AuthKind;
  keyPath: string | null;
}

export type ConnectOutcome =
  | { status: "connected"; warning: string | null }
  | { status: "hostKeyUnknown"; token: string; keyType: string; sha256: string };

export type CredentialKind = "password" | "passphrase";

export type IpcError =
  | { kind: "validation"; field: string; message: string }
  | { kind: "notFound" }
  | { kind: "unreachable"; message: string }
  | { kind: "connectTimeout" }
  | { kind: "authTimeout" }
  | {
      kind: "hostKeyMismatch";
      pinnedKeyType: string;
      pinnedSha256: string;
      presentedKeyType: string;
      presentedSha256: string;
      replaceToken: string;
    }
  | { kind: "authFailed" }
  | { kind: "credentialRequired"; credential: CredentialKind }
  | { kind: "keyFileUnreadable"; path: string }
  | { kind: "notConnected" }
  | { kind: "commandTimeout" }
  | { kind: "cancelled" }
  | { kind: "connectionLost" }
  | { kind: "keychain"; message: string }
  | { kind: "storage"; message: string }
  | { kind: "hostKeyApproval"; message: string }
  | { kind: "alreadyPinned" }
  | { kind: "confirmationMismatch" }
  | { kind: "protocol"; message: string }
  | { kind: "operationFailed"; message: string }
  | { kind: "internal" };

export interface OsInfo {
  id: string | null;
  versionId: string | null;
  prettyName: string | null;
}

export interface DiskInfo {
  mount: string;
  totalBytes: number;
  usedBytes: number;
  availableBytes: number;
}

export interface ProbeError {
  probe: string;
  message: string;
}

export interface ServerInfo {
  observedAt: string;
  hostname: string | null;
  os: OsInfo | null;
  kernel: string | null;
  arch: string | null;
  uptimeSeconds: number | null;
  cpuCount: number | null;
  cpuModel: string | null;
  memoryTotalKb: number | null;
  memoryAvailableKb: number | null;
  rootFs: DiskInfo | null;
  probeErrors: ProbeError[];
}

export interface OperationInfo {
  id: string;
  description: string;
}

export interface ModuleInfo {
  id: string;
  name: string;
  version: string;
  description: string;
  dependencies: string[];
  operations: OperationInfo[];
}
