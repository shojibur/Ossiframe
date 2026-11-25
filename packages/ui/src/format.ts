import type { IpcError, PlanReason } from "@ossiframe/client";

export function gib(bytes: number): string {
  return `${(bytes / 1024 ** 3).toFixed(1)} GiB`;
}

/** Used percentage as `df` shows it: ceil(used * 100 / (used + available)). Null when both are 0. */
export function diskUsedPercent(usedBytes: number, availableBytes: number): number | null {
  const denom = usedBytes + availableBytes;
  if (!(denom > 0)) return null;
  return Math.ceil((usedBytes * 100) / denom);
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

/** `detail` is the backend's short sanitized text and may add specifics. */
function planMessage(reason: PlanReason, detail: string): string {
  const more = detail ? ` ${detail}` : "";
  switch (reason) {
    case "unknownPlan":
      return "This plan no longer exists. Plans are kept in memory only and each can be approved once. Create a new plan.";
    case "superseded":
      return "A newer plan was created for this server. Review and approve the newest plan.";
    case "expired":
      return "The plan expired. Nothing was changed on the server. Create a new plan.";
    case "mismatch":
      return `The request no longer matches what was reviewed (plan content or lock holder). Nothing was changed by this request.${more}`;
    case "wrongServer":
      return `The connected server does not match the one this plan was made for. Nothing was changed.${more}`;
    case "busy":
      return `Another operation holds this server.${more}`;
    case "serverInUse":
      return "This server has an operation that is running or needs review. Resolve it first, then retry.";
    case "blocked":
      return `This cannot go ahead because of a blocking finding.${more}`;
    case "rePlanRequired":
      return `The server changed after the plan was made. Nothing was changed. Create a new plan.${more}`;
    case "notSupported":
      return `This is not supported.${more}`;
    case "appNotFound":
      return "This application no longer exists.";
    case "domainInUse":
      return `This domain is already used by another application.${more}`;
    case "slugInUse":
      return "An application with this name already exists on this server.";
    case "gitHostKeyUnconfirmed":
      return "Confirm the Git host's key fingerprint before continuing.";
  }
}

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
    case "operationFailed":
      return `The operation failed: ${e.message}`;
    case "plan":
      return planMessage(e.reason, e.message);
    case "internal":
      return "Something went wrong.";
  }
}
