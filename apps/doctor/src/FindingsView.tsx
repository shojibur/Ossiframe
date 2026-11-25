// Findings list with filters, search and a detail panel. Used by Diagnostics (all categories)
// and Security (a category preset). All strings from the server or the audit are rendered as
// text; nothing here uses HTML injection.
import { useMemo, useState } from "react";
import { Input, KeyValue, Notice, SeverityBadge, StatusBadge, cn, localTime, type Tone } from "@ossiframe/ui";
import type { Category, CheckResult, Outcome, Severity } from "./audit-types";
import {
  coverageLabel,
  incompleteLabel,
  type ScanEntry,
  useScanEntry,
  useServerEndpoint,
} from "./scan-store";

interface Row {
  key: string;
  step: string;
  check: CheckResult;
}

const SEVERITIES: Severity[] = ["critical", "high", "medium", "low", "info"];
const SEVERITY_RANK: Record<Severity, number> = { critical: 0, high: 1, medium: 2, low: 3, info: 4 };
const OUTCOMES: { id: Outcome; label: string }[] = [
  { id: "finding", label: "Finding" },
  { id: "passed", label: "Passed" },
  { id: "unavailable", label: "Unavailable" },
  { id: "incomplete", label: "Incomplete" },
  { id: "error", label: "Error" },
  { id: "notApplicable", label: "Not applicable" },
];
const OUTCOME_LABEL = Object.fromEntries(OUTCOMES.map((o) => [o.id, o.label])) as Record<Outcome, string>;
const OUTCOME_TONE: Record<Outcome, Tone> = {
  passed: "ok",
  finding: "warn",
  unavailable: "neutral",
  incomplete: "neutral",
  notApplicable: "neutral",
  error: "error",
};
const OUTCOME_ORDER: Record<Outcome, number> = {
  finding: 0,
  error: 1,
  incomplete: 2,
  unavailable: 3,
  passed: 4,
  notApplicable: 5,
};

function rowsOf(entry: ScanEntry, categories: Category[] | undefined): Row[] {
  const rows: Row[] = [];
  for (const step of entry.steps) {
    (entry.results[step]?.checks ?? []).forEach((check, i) => {
      if (categories && !categories.includes(check.category)) return;
      rows.push({ key: `${step}/${check.ruleId}/${check.resource ?? ""}/${i}`, step, check });
    });
  }
  return rows.sort((a, b) => {
    const o = OUTCOME_ORDER[a.check.outcome] - OUTCOME_ORDER[b.check.outcome];
    if (o !== 0) return o;
    return SEVERITY_RANK[a.check.severity ?? "info"] - SEVERITY_RANK[b.check.severity ?? "info"];
  });
}

function cap(s: string): string {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

function matches(row: Row, q: string): boolean {
  if (!q) return true;
  const c = row.check;
  return [c.title, c.ruleId, c.resource ?? "", c.explanation, c.category].some((s) => s.toLowerCase().includes(q));
}

const selectClass =
  "h-9 rounded-md border border-slate-300 bg-white px-2 text-sm text-slate-900 focus-visible:outline-2 focus-visible:outline-sky-600 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-100";

function Count({ label, value }: { label: string; value: number }) {
  return (
    <span className="rounded border border-slate-200 px-2 py-0.5 text-xs text-slate-700 dark:border-slate-800 dark:text-slate-300">
      {label} <span className="font-semibold tabular-nums">{value}</span>
    </span>
  );
}

function Detail({ row, stale }: { row: Row; stale: boolean }) {
  const c = row.check;
  return (
    <div className="flex flex-col gap-3 rounded-lg border border-slate-200 p-4 dark:border-slate-800">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="text-sm font-semibold">{c.title}</h3>
        {c.severity && <SeverityBadge severity={c.severity} />}
        <StatusBadge tone={OUTCOME_TONE[c.outcome]}>{OUTCOME_LABEL[c.outcome]}</StatusBadge>
        {c.inferred && (
          <span
            className="rounded border border-violet-400 bg-violet-50 px-1.5 py-0.5 text-xs font-medium text-violet-900 dark:bg-violet-950 dark:text-violet-200"
            title="This conclusion goes beyond what was directly observed."
          >
            Inferred
          </span>
        )}
      </div>
      <KeyValue
        rows={[
          ["Rule", c.ruleId],
          ["Category", c.category],
          ["Resource", c.resource ?? "none"],
          ["Observed", `${localTime(c.observedAt)}${stale ? " (server not connected now)" : ""}`],
        ]}
      />
      <section>
        <h4 className="text-xs font-semibold text-slate-500 uppercase dark:text-slate-400">Explanation</h4>
        <p className="mt-1 text-sm break-words whitespace-pre-wrap">{c.explanation}</p>
      </section>
      {c.recommendation && (
        <section>
          <h4 className="text-xs font-semibold text-slate-500 uppercase dark:text-slate-400">Recommendation</h4>
          <p className="mt-1 text-sm break-words whitespace-pre-wrap">{c.recommendation}</p>
          <p className="mt-1 text-xs text-slate-500">Advice only. Doctor does not change the server.</p>
        </section>
      )}
      <section>
        <h4 className="text-xs font-semibold text-slate-500 uppercase dark:text-slate-400">Evidence</h4>
        {c.evidence.length === 0 ? (
          <p className="mt-1 text-sm text-slate-500">No evidence lines.</p>
        ) : (
          <pre className="mt-1 max-h-48 overflow-auto rounded border border-slate-200 bg-slate-50 p-2 font-mono text-xs break-all whitespace-pre-wrap dark:border-slate-800 dark:bg-slate-900">
            {c.evidence.join("\n")}
          </pre>
        )}
      </section>
      {c.limitations.length > 0 && (
        <section>
          <h4 className="text-xs font-semibold text-slate-500 uppercase dark:text-slate-400">Limitations</h4>
          <ul className="mt-1 list-disc pl-5 text-sm break-words">
            {c.limitations.map((l, i) => (
              <li key={i}>{l}</li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}

export interface FindingsViewProps {
  serverId: string;
  connected: boolean;
  /** Show only checks of these categories. All when omitted. */
  categories?: Category[];
  /** Noun for empty-state text, for example "diagnostic" or "security". */
  noun: string;
}

export function FindingsView({ serverId, connected, categories, noun }: FindingsViewProps) {
  const endpoint = useServerEndpoint(serverId, connected);
  // While the endpoint is unknown, "" never matches a stored endpoint, so nothing shows.
  const entry = useScanEntry(serverId, endpoint ?? "");
  const [severity, setSeverity] = useState<"all" | Severity>("all");
  const [outcome, setOutcome] = useState<"all" | Outcome>("all");
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<string | null>(null);

  const all = useMemo(() => rowsOf(entry, categories), [entry, categories]);
  const q = query.trim().toLowerCase();
  const visible = useMemo(
    () =>
      all.filter(
        (r) =>
          (outcome === "all" || r.check.outcome === outcome) &&
          (severity === "all" || (r.check.outcome === "finding" && r.check.severity === severity)) &&
          matches(r, q),
      ),
    [all, outcome, severity, q],
  );
  const counts = useMemo(() => {
    const o: Record<Outcome, number> = { passed: 0, finding: 0, unavailable: 0, incomplete: 0, notApplicable: 0, error: 0 };
    const s: Record<Severity, number> = { critical: 0, high: 0, medium: 0, low: 0, info: 0 };
    for (const r of all) {
      o[r.check.outcome] += 1;
      if (r.check.outcome === "finding" && r.check.severity) s[r.check.severity] += 1;
    }
    return { o, s };
  }, [all]);

  const detail = visible.find((r) => r.key === selected) ?? null;
  const incomplete = incompleteLabel(entry);
  const coverage = coverageLabel(entry);
  const errorSteps = Object.entries(entry.stepErrors);
  const hasScan = entry.scanId > 0;
  const filtered = outcome !== "all" || severity !== "all" || q !== "";

  if (!hasScan) {
    return (
      <p className="text-sm text-slate-500">
        {connected
          ? `No assessment has run for this server yet. Start a scan from the Overview page to see ${noun} results.`
          : "No assessment results for this server. Connect and run a scan from the Overview page."}
      </p>
    );
  }

  return (
    <div className="flex flex-col gap-4">
      {entry.stale && (
        <Notice tone="warn">
          These results may be out of date. The server is not connected now
          {entry.staleAt ? `; the connection was lost at ${localTime(entry.staleAt)}` : ""}. They were collected
          {entry.finishedAt ? ` at ${localTime(entry.finishedAt)}` : " earlier"}.
        </Notice>
      )}
      {incomplete && <Notice tone="warn">{incomplete}. Results below cover only the steps that ran.</Notice>}
      {errorSteps.map(([step, message]) => (
        <Notice key={step} tone="error">
          Step {step} failed: {message}
        </Notice>
      ))}
      {entry.state === "running" && (
        <p className="text-sm text-slate-500">
          Scan running{entry.current ? `: ${entry.current}` : ""}. Results appear as steps finish.
        </p>
      )}

      <div className="flex flex-wrap items-center gap-2">
        {coverage && <span className="text-xs font-medium text-slate-700 dark:text-slate-300">{coverage}</span>}
        {SEVERITIES.map((s) => (
          <Count key={s} label={cap(s)} value={counts.s[s]} />
        ))}
        <Count label="Passed" value={counts.o.passed} />
        <Count label="Not assessed" value={counts.o.unavailable + counts.o.incomplete + counts.o.error} />
        <Count label="Not applicable" value={counts.o.notApplicable} />
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <div className="w-64">
          <Input
            type="search"
            aria-label="Search checks"
            placeholder="Search title, rule, resource"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
        <select
          aria-label="Filter by severity"
          className={selectClass}
          value={severity}
          onChange={(e) => setSeverity(e.target.value as "all" | Severity)}
        >
          <option value="all">All severities</option>
          {SEVERITIES.map((s) => (
            <option key={s} value={s}>
              {cap(s)}
            </option>
          ))}
        </select>
        <select
          aria-label="Filter by outcome"
          className={selectClass}
          value={outcome}
          onChange={(e) => setOutcome(e.target.value as "all" | Outcome)}
        >
          <option value="all">All outcomes</option>
          {OUTCOMES.map((o) => (
            <option key={o.id} value={o.id}>
              {o.label}
            </option>
          ))}
        </select>
        <span className="text-xs text-slate-500">
          {visible.length} of {all.length}
        </span>
      </div>

      {all.length === 0 ? (
        <p className="text-sm text-slate-500">
          {entry.state === "running"
            ? `No ${noun} results yet.`
            : `This scan produced no ${noun} results. That does not mean the server is free of issues.`}
        </p>
      ) : visible.length === 0 ? (
        <p className="text-sm text-slate-500">{filtered ? "No checks match the current filters." : "Nothing to show."}</p>
      ) : (
        <div className="grid gap-4 lg:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
          <ul className="flex flex-col divide-y divide-slate-200 rounded-lg border border-slate-200 dark:divide-slate-800 dark:border-slate-800">
            {visible.map((r) => (
              <li key={r.key}>
                <button
                  type="button"
                  aria-pressed={r.key === selected}
                  onClick={() => setSelected(r.key === selected ? null : r.key)}
                  className={cn(
                    "flex w-full flex-col gap-1 px-3 py-2 text-left text-sm hover:bg-slate-50 focus-visible:outline-2 focus-visible:outline-sky-600 dark:hover:bg-slate-900",
                    r.key === selected && "bg-slate-100 dark:bg-slate-900",
                  )}
                >
                  <span className="flex flex-wrap items-center gap-2">
                    {r.check.severity && <SeverityBadge severity={r.check.severity} />}
                    <StatusBadge tone={OUTCOME_TONE[r.check.outcome]}>{OUTCOME_LABEL[r.check.outcome]}</StatusBadge>
                    {r.check.inferred && <span className="text-xs text-violet-700 dark:text-violet-300">Inferred</span>}
                  </span>
                  <span className="font-medium break-words">{r.check.title}</span>
                  <span className="text-xs break-all text-slate-500">
                    {r.check.category}
                    {r.check.resource ? ` · ${r.check.resource}` : ""}
                  </span>
                </button>
              </li>
            ))}
          </ul>
          <div>
            {detail ? (
              <Detail row={detail} stale={entry.stale} />
            ) : (
              <p className="text-sm text-slate-500">Select a check to see its evidence and explanation.</p>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
