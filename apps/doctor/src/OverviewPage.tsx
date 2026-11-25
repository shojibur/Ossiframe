// Doctor Overview: assessment controls and summary above the inventory overview.
import type { ReactNode } from "react";
import { Button, Notice, SeverityBadge, StatusBadge, localTime, type Tone } from "@ossiframe/ui";
import { Overview as InventoryOverview } from "../../../modules/linux-inventory/ui/Overview";
import type { Severity } from "./audit-types";
import {
  cancelScan,
  coverageLabel,
  incompleteLabel,
  scanStatus,
  startScan,
  summarize,
  useScanEntry,
  useServerEndpoint,
} from "./scan-store";

const SEVERITIES: Severity[] = ["critical", "high", "medium", "low", "info"];

function Card({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="flex min-w-40 flex-1 flex-col gap-1.5 rounded-lg border border-slate-200 p-3 dark:border-slate-800">
      <h3 className="text-xs font-semibold text-slate-500 uppercase dark:text-slate-400">{title}</h3>
      {children}
    </div>
  );
}

function Num({ label, value }: { label: string; value: number }) {
  return (
    <div className="flex items-center justify-between gap-2 text-sm">
      <span>{label}</span>
      <span className="font-semibold tabular-nums">{value}</span>
    </div>
  );
}

function ScanPanel({ serverId, connected }: { serverId: string; connected: boolean }) {
  const endpoint = useServerEndpoint(serverId, connected);
  const entry = useScanEntry(serverId, endpoint ?? "");
  const running = entry.state === "running";
  const sum = summarize(entry);
  const status = scanStatus(entry);
  const incomplete = incompleteLabel(entry);
  const coverage = coverageLabel(entry);
  const total = entry.steps.length;
  const index = entry.current ? entry.steps.indexOf(entry.current) + 1 : 0;
  const errorSteps = Object.entries(entry.stepErrors);
  const statusTone: Tone = running ? "busy" : status === "complete" ? "ok" : status === "incomplete" ? "warn" : "neutral";
  const statusText = running ? "Running" : status === "complete" ? "Complete" : status === "incomplete" ? "Incomplete" : "Not run";
  return (
    <section className="flex flex-col gap-3 rounded-lg border border-slate-200 p-4 dark:border-slate-800">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 className="text-sm font-semibold">Assessment</h2>
        <div className="flex items-center gap-2">
          {running ? (
            <Button variant="secondary" disabled={entry.cancelling} onClick={() => void cancelScan(serverId)}>
              {entry.cancelling ? "Cancelling…" : "Cancel"}
            </Button>
          ) : (
            <Button disabled={!connected} onClick={() => void startScan(serverId)}>
              Run assessment
            </Button>
          )}
        </div>
      </div>
      <p className="text-xs text-slate-500">
        Read-only checks with predefined commands. Nothing on the server is changed. Results are kept in memory only.
      </p>
      {running && (
        <p className="text-sm" role="status">
          {total > 0 ? `Step ${Math.max(index, 1)} of ${total}` : "Starting"}
          {entry.current ? `: ${entry.current}` : ""}
        </p>
      )}
      {entry.stale && (
        <Notice tone="warn">
          Results may be out of date: the server is not connected now
          {entry.staleAt ? ` (connection lost ${localTime(entry.staleAt)})` : ""}.
        </Notice>
      )}
      {incomplete && <Notice tone="warn">{incomplete}.</Notice>}
      {errorSteps.map(([step, message]) => (
        <Notice key={step} tone="error">
          Step {step} failed: {message}
        </Notice>
      ))}
      {entry.scanId === 0 ? (
        <p className="text-sm text-slate-500">
          {connected ? "No assessment has run for this server yet." : "Connect to the server to run an assessment."}
        </p>
      ) : (
        <>
          <div className="flex flex-wrap items-center gap-3 text-sm">
            <StatusBadge tone={statusTone}>Scan status: {statusText}</StatusBadge>
            {coverage && <span className="text-slate-700 dark:text-slate-300">{coverage}</span>}
            {entry.finishedAt && <span className="text-xs text-slate-500">Finished {localTime(entry.finishedAt)}</span>}
          </div>
          <div className="flex flex-wrap gap-3">
            <Card title="Findings by severity">
              {SEVERITIES.map((s) => (
                <div key={s} className="flex items-center justify-between gap-2 text-sm">
                  <SeverityBadge severity={s} />
                  <span className="font-semibold tabular-nums">{sum.severities[s]}</span>
                </div>
              ))}
            </Card>
            <Card title="Coverage">
              <Num label="Assessed" value={sum.coverage.assessed} />
              <Num label="Not assessed" value={sum.coverage.notAssessed} />
              <Num label="Not applicable" value={sum.coverage.notApplicable} />
            </Card>
            <Card title="Outcomes">
              <Num label="Passed" value={sum.outcomes.passed} />
              <Num label="Findings" value={sum.outcomes.finding} />
              <Num label="Unavailable" value={sum.outcomes.unavailable} />
              <Num label="Incomplete" value={sum.outcomes.incomplete} />
              <Num label="Errors" value={sum.outcomes.error} />
            </Card>
          </div>
          <p className="text-xs text-slate-500">
            Coverage is separate from scan status. A finished scan can still leave checks not assessed, and no result
            here means the server is free of issues.
          </p>
        </>
      )}
    </section>
  );
}

export function DoctorOverview({ serverId, connected }: { serverId: string; connected: boolean }) {
  return (
    <div className="flex flex-col gap-5">
      <ScanPanel serverId={serverId} connected={connected} />
      <InventoryOverview serverId={serverId} connected={connected} />
    </div>
  );
}
