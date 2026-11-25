import type { ReactNode } from "react";
import { api, type DiskInfo, type ServerInfo } from "@ossiframe/client";
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

function pct(fs: DiskInfo): number {
  return fs.totalBytes ? Math.round((fs.usedBytes / fs.totalBytes) * 100) : 0;
}

function swapText(info: ServerInfo): string | null {
  if (info.swapTotalKb === null) return null;
  if (info.swapTotalKb === 0) return "none configured";
  const free = info.swapFreeKb !== null ? kbToGib(info.swapFreeKb) : "unknown";
  return `${kbToGib(info.swapTotalKb)} total, ${free} free`;
}

function Filesystems({ info }: { info: ServerInfo }) {
  if (info.filesystems.length === 0) {
    return <p className="text-sm text-slate-500">No filesystems reported.</p>;
  }
  return (
    <div className="flex flex-col gap-2">
      <div className="overflow-x-auto rounded-md border border-slate-200 dark:border-slate-800">
        <table className="w-full text-left text-sm">
          <thead className="bg-slate-50 text-xs text-slate-500 uppercase dark:bg-slate-900 dark:text-slate-400">
            <tr>
              <th className="px-3 py-1.5 font-medium">Mount</th>
              <th className="px-3 py-1.5 text-right font-medium">Total</th>
              <th className="px-3 py-1.5 text-right font-medium">Used</th>
              <th className="px-3 py-1.5 text-right font-medium">Available</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-slate-200 dark:divide-slate-800">
            {info.filesystems.map((f, i) => (
              <tr key={`${f.mount}/${i}`}>
                <td className="px-3 py-1.5 font-mono text-[13px] break-all">{f.mount}</td>
                <td className="px-3 py-1.5 text-right tabular-nums">{gib(f.totalBytes)}</td>
                <td className="px-3 py-1.5 text-right tabular-nums">
                  {gib(f.usedBytes)} ({pct(f)}%)
                </td>
                <td className="px-3 py-1.5 text-right tabular-nums">{gib(f.availableBytes)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {info.filesystemsTruncated && (
        <p className="text-xs text-slate-500">The filesystem list is truncated; some filesystems are not shown.</p>
      )}
    </div>
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
    ["Swap", swapText(info) ?? u(["swap", "memory"])],
    [
      "CPU usage",
      info.cpuUsagePercent !== null ? (
        `${info.cpuUsagePercent.toFixed(0)}% (about one second sample)`
      ) : (
        <span className="text-slate-500 italic">not measured</span>
      ),
    ],
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
          <h3 className="text-xs font-semibold text-slate-500 uppercase dark:text-slate-400">Filesystems</h3>
          <Filesystems info={info.info} />
        </div>
      )}
    </div>
  );
}
