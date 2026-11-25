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

export type PlanReason =
  | "unknownPlan"
  | "superseded"
  | "expired"
  | "mismatch"
  | "wrongServer"
  | "busy"
  | "serverInUse"
  | "blocked"
  | "rePlanRequired"
  | "notSupported"
  | "appNotFound"
  | "domainInUse"
  | "slugInUse"
  | "gitHostKeyUnconfirmed";

/** A scanned SSH host key of a Git host. `sha256` is `SHA256:...`. */
export interface GitHostFingerprint {
  host: string;
  port: number;
  keyType: string;
  sha256: string;
}

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
  | {
      kind: "plan";
      reason: PlanReason;
      message: string;
      /** Only present for `gitHostKeyUnconfirmed`. */
      fingerprints?: GitHostFingerprint[];
    }
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
  swapTotalKb: number | null;
  swapFreeKb: number | null;
  cpuUsagePercent: number | null;
  rootFs: DiskInfo | null;
  filesystems: DiskInfo[];
  filesystemsTruncated: boolean;
  limitations: string[];
  probeErrors: ProbeError[];
}

export interface Service {
  unit: string;
  description: string | null;
  installed: boolean;
  load: string;
  active: string;
  sub: string;
  enablement: string;
}

export interface ServicesInfo {
  observedAt: string;
  available: boolean;
  services: Service[];
  truncated: boolean;
  unrecognizedRecords: number;
  limitations: string[];
  probeErrors: ProbeError[];
}

export type SocketProtocol = "tcp" | "udp";
export type SocketFamily = "ipv4" | "ipv6" | "unknown";
export type SocketBinding = "loopback" | "wildcard" | "specific";

export interface SocketProcess {
  name: string;
  pid: number;
}

export interface ListeningSocket {
  protocol: SocketProtocol;
  address: string;
  port: number;
  family: SocketFamily;
  binding: SocketBinding;
  private: boolean;
  process: SocketProcess | null;
}

export interface SocketsInfo {
  observedAt: string;
  available: boolean;
  sockets: ListeningSocket[];
  truncated: boolean;
  limitations: string[];
  probeErrors: ProbeError[];
}

export interface Package {
  name: string;
  version: string;
}

export interface SoftwareInfo {
  observedAt: string;
  available: boolean;
  packages: Package[];
  truncated: boolean;
  limitations: string[];
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
