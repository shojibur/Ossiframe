import type { ReactNode } from "react";
import { api, type ServerInfo } from "@ossiframe/client";
import { Button, KeyValue, Notice, duration, gib, kbToGib, localTime } from "@ossiframe/ui";
import { loadServerInfo, useServerEntry } from "./info-store";

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

export function Overview({ serverId, connected }: { serverId: string; connected: boolean }) {
  const { info, loading, error } = useServerEntry(serverId);
  return (
    <div className="flex flex-col gap-5">
      {connected && (
        <div className="flex gap-2">
          {loading ? (
            <Button variant="secondary" onClick={() => void api.cancelOperation(serverId)}>
              Cancel
            </Button>
          ) : (
            <Button onClick={() => void loadServerInfo(serverId)}>Refresh</Button>
          )}
        </div>
      )}

      {error && <Notice tone="error">{error}</Notice>}

      {loading && !info && <p className="text-sm text-slate-500">Reading server information…</p>}
      {!info && !loading && !connected && <p className="text-sm text-slate-500">Connect to read live server information.</p>}

      {info && (
        <div className="flex flex-col gap-3 rounded-lg border border-slate-200 p-4 dark:border-slate-800">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <h2 className="text-sm font-semibold">System</h2>
            <span className="text-xs text-slate-500">Observed {localTime(info.info.observedAt)}</span>
          </div>
          {info.stale && (
            <Notice tone="warn">This information may be out of date: it was observed earlier and the server is not connected now.</Notice>
          )}
          <KeyValue rows={rows(info.info)} />
        </div>
      )}
    </div>
  );
}
