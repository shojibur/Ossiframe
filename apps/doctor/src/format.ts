import type { IpcError } from "@ossiframe/client";

export function gib(bytes: number): string {
  return `${(bytes / 1024 ** 3).toFixed(1)} GiB`;
}

export function kbToGib(kb: number): string {
  return gib(kb * 1024);
}

export function duration(totalSeconds: number): string {
  const d = Math.floor(totalSeconds / 86400);
  const h = Math.floor((totalSeconds % 86400) / 3600);
  const m = Math.floor((totalSeconds % 3600) / 60);
  return [d ? `${d}d` : "", h ? `${h}h` : "", `${m}m`].filter(Boolean).join(" ");
}

export function localTime(iso: string): string {
  const t = new Date(iso);
  return Number.isNaN(t.getTime()) ? iso : t.toLocaleString();
}

const MAY_STILL_RUN = "The remote command may still be running on the server. It was not retried.";

/** Plain-text message for an IPC error. */
export function errorMessage(e: IpcError): string {
  switch (e.kind) {
    case "validation":
      return `${e.field}: ${e.message}`;
    case "notFound":
      return "This server profile no longer exists.";
    case "unreachable":
      return `Cannot reach the server: ${e.message}`;
    case "connectTimeout":
      return "The connection timed out. Check the host, port and network.";
    case "hostKeyMismatch":
      return "The server's host key has changed. The connection was blocked.";
    case "authTimeout":
      return "Authentication timed out. The connection was closed.";
    case "authFailed":
      return "Authentication failed.";
    case "credentialRequired":
      return e.credential === "password" ? "A password is required." : "The private key needs its passphrase.";
    case "keyFileUnreadable":
      return `Cannot read the private key file at ${e.path}.`;
    case "notConnected":
      return "Not connected. Connect to the server first.";
    case "commandTimeout":
      return `A command timed out. ${MAY_STILL_RUN}`;
    case "cancelled":
      return `Cancelled. ${MAY_STILL_RUN}`;
    case "connectionLost":
      return `The connection was lost. ${MAY_STILL_RUN}`;
    case "keychain":
      return `Keychain: ${e.message}`;
    case "storage":
      return `Local storage error: ${e.message}`;
    case "hostKeyApproval":
      return e.message;
    case "alreadyPinned":
      return "A host key is already pinned for this server.";
    case "confirmationMismatch":
      return "The typed host name does not match.";
    case "protocol":
      return `SSH error: ${e.message}`;
    case "internal":
      return "Something went wrong.";
  }
}
