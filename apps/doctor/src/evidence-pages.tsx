// Services, Network and Software pages. They read the evidence of the latest assessment for
// this server and endpoint; they never run a command themselves. All remote strings are
// rendered as text.
import { useMemo, useState, type ReactNode } from "react";
import type { ListeningSocket, Package, Service } from "@ossiframe/client";
import { Input, Notice, localTime } from "@ossiframe/ui";
import type { NetworkEvidence, ServicesEvidence, SoftwareEvidence, StepResult } from "./audit-types";
import { useScanEntry, useServerEndpoint, type ScanEntry } from "./scan-store";

const selectClass =
  "h-9 rounded-md border border-slate-300 bg-white px-2 text-sm text-slate-900 focus-visible:outline-2 focus-visible:outline-sky-600 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-100";
const TH = "px-3 py-1.5 font-medium";
const TD = "px-3 py-1.5 align-top break-words";

interface Source<E> {
  entry: ScanEntry;
  result: StepResult<E> | null;
}

function useStep<E>(serverId: string, connected: boolean, step: string): Source<E> {
  const endpoint = useServerEndpoint(serverId, connected);
  // While the endpoint is unknown, "" never matches a stored endpoint, so nothing shows.
  const entry = useScanEntry(serverId, endpoint ?? "");
  return { entry, result: (entry.results[step] as StepResult<E> | undefined) ?? null };
}

/** Handles the states in which there is no usable list. Returns `null` when the table can show. */
function emptyState(
  { entry, result }: Source<{ available: boolean; limitations: string[] }>,
  connected: boolean,
  step: string,
  noun: string,
): ReactNode | null {
  if (entry.scanId === 0) {
    return (
      <p className="text-sm text-slate-500">
        {connected
          ? `No assessment has run for this server yet. Run an assessment from the Overview page to see ${noun}.`
          : `No assessment results for this server. Connect and run an assessment from the Overview page to see ${noun}.`}
      </p>
    );
  }
  if (!result) {
    if (entry.state === "running") return <p className="text-sm text-slate-500">Waiting for the {step} step to finish…</p>;
    const err = entry.stepErrors[step];
    return (
      <Notice tone={err ? "error" : "warn"}>
        {err ? `The ${step} step failed: ${err}` : `The ${step} step did not run in the latest assessment.`} {entry.stale ? "" : "Run the assessment again to collect it."}
      </Notice>
    );
  }
  if (!result.evidence.available) {
    const why = result.evidence.limitations?.[0] ?? result.checks[0]?.explanation;
    return (
      <Notice tone="warn">
        {noun.charAt(0).toUpperCase() + noun.slice(1)} are unavailable: the data could not be read on the server.
        {why ? ` ${why}` : ""}
      </Notice>
    );
  }
  return null;
}

function Banners({
  entry,
  result,
  truncated,
  limitations,
}: {
  entry: ScanEntry;
  result: StepResult;
  truncated: boolean;
  limitations: string[];
}) {
  return (
    <>
      {entry.stale && (
        <Notice tone="warn">
          This data may be out of date: it was observed {localTime(result.observedAt)} and the server is not connected now.
        </Notice>
      )}
      {truncated && <Notice tone="warn">The list is truncated: some entries are not shown, so absence from this list proves nothing.</Notice>}
      {limitations.length > 0 && (
        <ul className="list-disc pl-5 text-xs text-slate-500">
          {limitations.map((l, i) => (
            <li key={i}>{l}</li>
          ))}
        </ul>
      )}
    </>
  );
}

function Chip({ label, value }: { label: string; value: number }) {
  return (
    <span className="rounded border border-slate-200 px-2 py-0.5 text-xs text-slate-700 dark:border-slate-800 dark:text-slate-300">
      {label} <span className="font-semibold tabular-nums">{value}</span>
    </span>
  );
}

function Table({ head, children, empty }: { head: string[]; children: ReactNode[]; empty: string }) {
  if (children.length === 0) return <p className="text-sm text-slate-500">{empty}</p>;
  return (
    <div className="overflow-x-auto rounded-md border border-slate-200 dark:border-slate-800">
      <table className="w-full text-left text-sm">
        <thead className="bg-slate-50 text-xs text-slate-500 uppercase dark:bg-slate-900 dark:text-slate-400">
          <tr>
            {head.map((h) => (
              <th key={h} className={TH}>
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody className="divide-y divide-slate-200 dark:divide-slate-800">{children}</tbody>
      </table>
    </div>
  );
}

// ---- Services ----

type ServiceFilter = "all" | "failed" | "active" | "inactive" | "enabled" | "notLoaded" | "unknown";
const SERVICE_FILTERS: { id: ServiceFilter; label: string }[] = [
  { id: "all", label: "All services" },
  { id: "failed", label: "Failed" },
  { id: "active", label: "Active" },
  { id: "inactive", label: "Inactive" },
  { id: "enabled", label: "Enabled" },
  { id: "notLoaded", label: "Installed, not loaded" },
  { id: "unknown", label: "Has unknown state" },
];

const hasUnknown = (s: Service) => s.load === "unknown" || s.active === "unknown" || s.sub === "unknown" || s.enablement === "unknown";

function serviceMatches(s: Service, f: ServiceFilter): boolean {
  switch (f) {
    case "all":
      return true;
    case "failed":
      return s.active === "failed";
    case "active":
      return s.active === "active";
    case "inactive":
      return s.active === "inactive";
    case "enabled":
      return s.enablement === "enabled";
    case "notLoaded":
      return s.installed && s.load === "unknown";
    case "unknown":
      return hasUnknown(s);
  }
}

export function ServicesPage({ serverId, connected }: { serverId: string; connected: boolean }) {
  const src = useStep<ServicesEvidence>(serverId, connected, "services");
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<ServiceFilter>("all");
  const list = useMemo(() => src.result?.evidence.services ?? [], [src.result]);
  const q = query.trim().toLowerCase();
  const visible = useMemo(
    () =>
      list.filter(
        (s) => serviceMatches(s, filter) && (!q || s.unit.toLowerCase().includes(q) || (s.description ?? "").toLowerCase().includes(q)),
      ),
    [list, filter, q],
  );
  const blocked = emptyState(src, connected, "services", "services");
  if (blocked || !src.result) return blocked;
  const ev = src.result.evidence;
  const count = (f: ServiceFilter) => list.filter((s) => serviceMatches(s, f)).length;
  return (
    <div className="flex flex-col gap-4">
      <Banners entry={src.entry} result={src.result} truncated={ev.truncated} limitations={ev.limitations ?? []} />
      <div className="flex flex-wrap items-center gap-2">
        <Chip label="Units (loaded ∪ installed)" value={list.length} />
        <Chip label="Installed" value={list.filter((s) => s.installed).length} />
        <Chip label="Loaded" value={list.filter((s) => s.load === "loaded").length} />
        <Chip label="Active" value={count("active")} />
        <Chip label="Failed" value={count("failed")} />
        <Chip label="Enabled" value={count("enabled")} />
        <Chip label="Unknown state" value={count("unknown")} />
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <div className="w-64">
          <Input type="search" aria-label="Search services" placeholder="Search unit or description" value={query} onChange={(e) => setQuery(e.target.value)} />
        </div>
        <select aria-label="Filter services" className={selectClass} value={filter} onChange={(e) => setFilter(e.target.value as ServiceFilter)}>
          {SERVICE_FILTERS.map((f) => (
            <option key={f.id} value={f.id}>
              {f.label}
            </option>
          ))}
        </select>
        <span className="text-xs text-slate-500">
          {visible.length} of {list.length}
        </span>
        <span className="text-xs text-slate-500">Observed {localTime(src.result.observedAt)}</span>
      </div>
      <Table
        head={["Unit", "Description", "Installed", "Load", "Active", "Sub", "Enabled"]}
        empty={list.length === 0 ? "systemd reported no services." : "No services match the current filters."}
      >
        {visible.map((s, i) => (
          <tr key={`${s.unit}/${i}`}>
            <td className={`${TD} font-mono text-[13px] break-all`}>{s.unit}</td>
            <td className={TD}>{s.description ?? <span className="text-slate-500">none</span>}</td>
            <td className={TD}>{s.installed ? "yes" : "no"}</td>
            <td className={TD}>{s.load}</td>
            <td className={TD}>{s.active}</td>
            <td className={TD}>{s.sub}</td>
            <td className={TD}>{s.enablement}</td>
          </tr>
        ))}
      </Table>
      <p className="text-xs text-slate-500">
        &quot;unknown&quot; means systemd reported no value, for example for an installed unit that is not loaded.
      </p>
    </div>
  );
}

// ---- Network ----

const BINDING_LABEL: Record<ListeningSocket["binding"], string> = {
  loopback: "Loopback",
  wildcard: "Wildcard (all addresses)",
  specific: "Specific address",
};

export function NetworkPage({ serverId, connected }: { serverId: string; connected: boolean }) {
  const src = useStep<NetworkEvidence>(serverId, connected, "network");
  const [query, setQuery] = useState("");
  const [binding, setBinding] = useState<"all" | ListeningSocket["binding"]>("all");
  const [protocol, setProtocol] = useState<"all" | ListeningSocket["protocol"]>("all");
  const list = useMemo(() => src.result?.evidence.sockets ?? [], [src.result]);
  const q = query.trim().toLowerCase();
  const visible = useMemo(
    () =>
      list.filter(
        (s) =>
          (binding === "all" || s.binding === binding) &&
          (protocol === "all" || s.protocol === protocol) &&
          (!q || `${s.address} ${s.port} ${s.process?.name ?? ""} ${s.process?.pid ?? ""}`.toLowerCase().includes(q)),
      ),
    [list, binding, protocol, q],
  );
  const blocked = emptyState(src, connected, "network", "listening sockets");
  if (blocked || !src.result) return blocked;
  const ev = src.result.evidence;
  const hidden = list.filter((s) => s.process === null).length;
  return (
    <div className="flex flex-col gap-4">
      <Banners entry={src.entry} result={src.result} truncated={ev.truncated} limitations={ev.limitations ?? []} />
      <div className="flex flex-wrap items-center gap-2">
        <Chip label="Listening" value={list.length} />
        <Chip label="Loopback" value={list.filter((s) => s.binding === "loopback").length} />
        <Chip label="Wildcard" value={list.filter((s) => s.binding === "wildcard").length} />
        <Chip label="Process not visible" value={hidden} />
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <div className="w-64">
          <Input type="search" aria-label="Search sockets" placeholder="Search address, port, process" value={query} onChange={(e) => setQuery(e.target.value)} />
        </div>
        <select aria-label="Filter by binding" className={selectClass} value={binding} onChange={(e) => setBinding(e.target.value as typeof binding)}>
          <option value="all">All bindings</option>
          {(Object.keys(BINDING_LABEL) as (keyof typeof BINDING_LABEL)[]).map((b) => (
            <option key={b} value={b}>
              {BINDING_LABEL[b]}
            </option>
          ))}
        </select>
        <select aria-label="Filter by protocol" className={selectClass} value={protocol} onChange={(e) => setProtocol(e.target.value as typeof protocol)}>
          <option value="all">TCP and UDP</option>
          <option value="tcp">TCP</option>
          <option value="udp">UDP</option>
        </select>
        <span className="text-xs text-slate-500">
          {visible.length} of {list.length}
        </span>
        <span className="text-xs text-slate-500">Observed {localTime(src.result.observedAt)}</span>
      </div>
      <Table
        head={["Protocol", "Address", "Port", "Family", "Binding", "Private", "Process"]}
        empty={list.length === 0 ? "No listening sockets were reported." : "No sockets match the current filters."}
      >
        {visible.map((s, i) => (
          <tr key={`${s.protocol}/${s.address}/${s.port}/${i}`}>
            <td className={TD}>{s.protocol.toUpperCase()}</td>
            <td className={`${TD} font-mono text-[13px] break-all`}>{s.address}</td>
            <td className={`${TD} tabular-nums`}>{s.port}</td>
            <td className={TD}>{s.family}</td>
            <td className={TD}>{BINDING_LABEL[s.binding]}</td>
            <td className={TD}>{s.private ? "yes" : "no"}</td>
            <td className={TD}>
              {s.process ? `${s.process.name} (${s.process.pid})` : <span className="text-slate-500 italic">not visible</span>}
            </td>
          </tr>
        ))}
      </Table>
      <p className="text-xs text-slate-500">
        &quot;Private&quot; means a loopback, private-range or link-local address. A process is not visible when the
        connected user may not read it. A non-loopback address is not proof that the port is reachable from the internet.
      </p>
    </div>
  );
}

// ---- Software ----

export function SoftwarePage({ serverId, connected }: { serverId: string; connected: boolean }) {
  const src = useStep<SoftwareEvidence>(serverId, connected, "software");
  const [query, setQuery] = useState("");
  const list: Package[] = useMemo(() => src.result?.evidence.packages ?? [], [src.result]);
  const q = query.trim().toLowerCase();
  const visible = useMemo(() => list.filter((p) => !q || p.name.toLowerCase().includes(q) || p.version.toLowerCase().includes(q)), [list, q]);
  const blocked = emptyState(src, connected, "software", "installed software");
  if (blocked || !src.result) return blocked;
  const ev = src.result.evidence;
  return (
    <div className="flex flex-col gap-4">
      <Notice tone="info">
        Detection uses the dpkg package database only. Software installed another way (source builds, containers, snap,
        language package managers) is not detected, and a package missing from this list may still be present.
      </Notice>
      <Banners entry={src.entry} result={src.result} truncated={ev.truncated} limitations={ev.limitations ?? []} />
      <div className="flex flex-wrap items-center gap-2">
        <div className="w-64">
          <Input type="search" aria-label="Search software" placeholder="Search package or version" value={query} onChange={(e) => setQuery(e.target.value)} />
        </div>
        <span className="text-xs text-slate-500">
          {visible.length} of {list.length}
        </span>
        <span className="text-xs text-slate-500">Observed {localTime(src.result.observedAt)}</span>
      </div>
      <Table
        head={["Package", "Version"]}
        empty={list.length === 0 ? "None of the checked server packages are installed through dpkg." : "No packages match the search."}
      >
        {visible.map((p, i) => (
          <tr key={`${p.name}/${i}`}>
            <td className={`${TD} font-mono text-[13px] break-all`}>{p.name}</td>
            <td className={`${TD} font-mono text-[13px] break-all`}>{p.version}</td>
          </tr>
        ))}
      </Table>
    </div>
  );
}
