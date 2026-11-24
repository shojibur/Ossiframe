import type { ReactNode } from "react";
import type { Server, ServerInfo } from "@ossiframe/client";
import { Button, KeyValue, Mono, Notice, StatusBadge, type Tone } from "@ossiframe/ui";
import { duration, gib, kbToGib, localTime } from "./format";

export interface InfoState {
  info: ServerInfo;
  stale: boolean;
}

const stateLabel: Record<Server["state"], [Tone, string]> = {
  connected: ["ok", "Connected"],
  connecting: ["busy", "Connecting"],
  disconnected: ["neutral", "Disconnected"],
  error: ["error", "Error"],
};

export function StateBadge({ server, busy }: { server: Server; busy: boolean }) {
  const [tone, label] = busy ? stateLabel.connecting : stateLabel[server.state];
  return <StatusBadge tone={tone}>{label}</StatusBadge>;
}

function unavailable(info: ServerInfo, probes: string[]): ReactNode {
  const err = info.probeErrors.find((e) => probes.includes(e.probe));
  return (
    <span className="text-slate-500 italic" title={err?.message}>
      unavailable{err ? ` (${err.message})` : ""}
    </span>
  );
}

function rows(info: ServerInfo): [string, ReactNode][] {
  const u = (p: string[]) => unavailable(info, p);
  const os = info.os ? (info.os.prettyName ?? [info.os.id, info.os.versionId].filter(Boolean).join(" ")) : null;
  const mem =
    info.memoryTotalKb !== null
      ? `${kbToGib(info.memoryTotalKb)} total, ${info.memoryAvailableKb !== null ? kbToGib(info.memoryAvailableKb) : "unknown"} available`
      : null;
  const fs = info.rootFs;
  const disk = fs
    ? `${gib(fs.totalBytes)} total, ${gib(fs.usedBytes)} used (${fs.totalBytes ? Math.round((fs.usedBytes / fs.totalBytes) * 100) : 0}%), ${gib(fs.availableBytes)} available`
    : null;
  return [
    ["Hostname", info.hostname ?? u(["hostname"])],
    ["Operating system", os ?? u(["os"])],
    ["Kernel", info.kernel ?? u(["kernel"])],
    ["Architecture", info.arch ?? u(["arch"])],
    ["Uptime", info.uptimeSeconds !== null ? duration(info.uptimeSeconds) : u(["uptime"])],
    ["CPU count", info.cpuCount ?? u(["cpuCount"])],
    ["CPU model", info.cpuModel ?? u(["cpuModel"])],
    ["Memory", mem ?? u(["memory"])],
    ["Root filesystem", disk ?? u(["rootFs"])],
  ];
}

interface Props {
  server: Server;
  info: InfoState | undefined;
  busy: boolean;
  loadingInfo: boolean;
  error: string | null;
  warning: string | null;
  onConnect: () => void;
  onDisconnect: () => void;
  onRefresh: () => void;
  onCancel: () => void;
  onEdit: () => void;
  onRemove: () => void;
}

export function Overview(p: Props) {
  const connected = p.server.state === "connected";
  return (
    <section className="flex flex-col gap-5 p-6">
      <header className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex flex-col gap-1">
          <h1 className="text-lg font-semibold">{p.server.name}</h1>
          <Mono className="text-slate-600 dark:text-slate-400">
            {p.server.username}@{p.server.host}:{p.server.port}
          </Mono>
          <StateBadge server={p.server} busy={p.busy} />
        </div>
        <div className="flex flex-wrap gap-2">
          {connected ? (
            <>
              {p.loadingInfo ? (
                <Button variant="secondary" onClick={p.onCancel}>
                  Cancel
                </Button>
              ) : (
                <Button onClick={p.onRefresh}>Refresh</Button>
              )}
              <Button variant="secondary" onClick={p.onDisconnect}>
                Disconnect
              </Button>
            </>
          ) : (
            <Button onClick={p.onConnect} disabled={p.busy}>
              Connect
            </Button>
          )}
          <Button variant="ghost" onClick={p.onEdit}>
            Edit
          </Button>
          <Button variant="ghost" onClick={p.onRemove}>
            Remove
          </Button>
        </div>
      </header>

      {p.error && <Notice tone="error">{p.error}</Notice>}
      {p.warning && <Notice tone="warn">{p.warning}</Notice>}

      <div className="text-xs text-slate-500">
        Pinned host key:{" "}
        {p.server.pinnedHostKey ? (
          <Mono>
            {p.server.pinnedHostKey.keyType} {p.server.pinnedHostKey.sha256}
          </Mono>
        ) : (
          "none yet (verified on first connection)"
        )}
      </div>

      {p.loadingInfo && !p.info && <p className="text-sm text-slate-500">Reading server information…</p>}
      {!p.info && !p.loadingInfo && !connected && <p className="text-sm text-slate-500">Connect to read live server information.</p>}

      {p.info && (
        <div className="flex flex-col gap-3 rounded-lg border border-slate-200 p-4 dark:border-slate-800">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <h2 className="text-sm font-semibold">System</h2>
            <span className="text-xs text-slate-500">Observed {localTime(p.info.info.observedAt)}</span>
          </div>
          {p.info.stale && (
            <Notice tone="warn">This information may be out of date: it was observed earlier and the server is not connected now.</Notice>
          )}
          <KeyValue rows={rows(p.info.info)} />
        </div>
      )}
    </section>
  );
}
