// Reports page. One snapshot of the latest assessment and one redaction setting are frozen when
// the user prepares a report. The preview (`build_report`) and both exports (`save_report`) send
// that same object, so what is previewed is what is saved. Report text is shown as text in a
// <pre>; it is never injected into the app DOM as HTML.
import { useRef, useState } from "react";
import { api, toIpcError } from "@ossiframe/client";
import { Button, Notice, errorMessage, localTime } from "@ossiframe/ui";
import type { ResourcesEvidence } from "./audit-types";
import { buildReport, saveReport, type ReportFormat, type ReportInput } from "./report-api";
import { incompleteLabel, coverageLabel, useScanEntry, useServerEndpoint } from "./scan-store";

interface Snapshot {
  report: ReportInput;
  scanId: number;
  redact: boolean;
}

interface Preview {
  format: ReportFormat;
  text: string;
}

type Feedback = { tone: "info" | "warn" | "error"; text: string } | null;

const CONTENTS = [
  "Profile name, port, and the connection address (unless omitted).",
  "Hostname, operating system and kernel as reported by the server.",
  "Check results with severity, outcome, evidence lines, explanation, recommendation and limitations.",
  "Memory and swap totals, filesystems, service counts, listening ports with process names, and installed package names and versions.",
  "Configuration file names and certificate paths read during the assessment.",
  "Scan status, coverage, timestamps, and whether the data is stale.",
];

export function ReportsPage({ serverId, connected }: { serverId: string; connected: boolean }) {
  const endpoint = useServerEndpoint(serverId, connected);
  const entry = useScanEntry(serverId, endpoint ?? "");

  const [redact, setRedact] = useState(true);
  const [format, setFormat] = useState<ReportFormat>("json");
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState<Feedback>(null);
  const ticket = useRef(0);

  const running = entry.state === "running";
  const hasScan = entry.scanId > 0 && !running && Object.keys(entry.results).length > 0;
  const incomplete = incompleteLabel(entry);
  const coverage = coverageLabel(entry);
  const outdated =
    snapshot !== null &&
    (snapshot.scanId !== entry.scanId ||
      snapshot.redact !== redact ||
      snapshot.report.scan.stale !== entry.stale);
  const canExport = snapshot !== null && preview !== null && !outdated && !busy;

  async function prepare() {
    const mine = ++ticket.current;
    setBusy(true);
    setFeedback(null);
    try {
      const server = (await api.listServers()).find((s) => s.id === serverId);
      if (!server) {
        setFeedback({ tone: "error", text: "Could not build the report: this server profile no longer exists." });
        return;
      }
      // Freeze: copy the results so a later scan or edit cannot change the snapshot.
      const steps = entry.steps.flatMap((s) => (entry.results[s] ? [structuredClone(entry.results[s])] : []));
      const identity =
        (steps.find((s) => s.step === "resources")?.evidence as Partial<ResourcesEvidence> | undefined) ?? null;
      const report: ReportInput = {
        server: {
          profileName: server.name,
          host: server.host,
          port: server.port,
          // Identity comes from the frozen `resources` step evidence, so it shares the assessment's
          // observation time. Null when that step is missing.
          hostname: identity?.hostname ?? null,
          os: identity?.os?.prettyName ?? identity?.os?.id ?? null,
          kernel: identity?.kernel ?? null,
        },
        scan: {
          startedAt: entry.startedAt ?? new Date().toISOString(),
          finishedAt: entry.finishedAt,
          completedSteps: steps.map((s) => s.step),
          stale: entry.stale,
        },
        steps,
        redactHost: redact,
      };
      const text = await buildReport(format, report);
      if (mine !== ticket.current) return;
      setSnapshot({ report, scanId: entry.scanId, redact });
      setPreview({ format, text });
    } catch (e) {
      if (mine !== ticket.current) return;
      setFeedback({ tone: "error", text: `Could not build the report: ${errorMessage(toIpcError(e))}` });
    } finally {
      if (mine === ticket.current) setBusy(false);
    }
  }

  async function changeFormat(next: ReportFormat) {
    setFormat(next);
    if (!snapshot || outdated) return;
    const mine = ++ticket.current;
    setBusy(true);
    setFeedback(null);
    try {
      const text = await buildReport(next, snapshot.report);
      if (mine !== ticket.current) return;
      setPreview({ format: next, text });
    } catch (e) {
      if (mine !== ticket.current) return;
      setFeedback({ tone: "error", text: `Could not build the report: ${errorMessage(toIpcError(e))}` });
    } finally {
      if (mine === ticket.current) setBusy(false);
    }
  }

  async function exportAs(as: ReportFormat) {
    if (!snapshot || outdated) return;
    setBusy(true);
    setFeedback(null);
    try {
      const saved = await saveReport(as, snapshot.report);
      setFeedback(
        saved
          ? { tone: "info", text: `Saved the ${as.toUpperCase()} report.` }
          : { tone: "info", text: "Export cancelled. Nothing was saved." },
      );
    } catch (e) {
      setFeedback({ tone: "error", text: `Could not save the report: ${errorMessage(toIpcError(e))}` });
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <section className="flex flex-col gap-2 rounded-lg border border-slate-200 p-4 dark:border-slate-800">
        <h2 className="text-sm font-semibold">Report</h2>
        <p className="text-sm text-slate-700 dark:text-slate-300">
          A report is a snapshot of the latest assessment of this server, saved as a JSON or HTML file. Review the
          report before you share it. Ossiframe does not check the text read from the server for sensitive data.
        </p>
        <h3 className="mt-1 text-xs font-semibold text-slate-500 uppercase dark:text-slate-400">What the report contains</h3>
        <ul className="list-disc pl-5 text-sm text-slate-700 dark:text-slate-300">
          {CONTENTS.map((c) => (
            <li key={c}>{c}</li>
          ))}
        </ul>
        <p className="text-xs text-slate-500">
          It does not contain passwords, passphrases, key files or key paths, or the SSH username.
        </p>
      </section>

      {entry.scanId === 0 || running || !hasScan ? (
        <Notice tone="warn">
          {running
            ? "An assessment is running. Wait for it to finish or cancel it to create a report."
            : connected
              ? "No assessment results for this server yet. Run an assessment from the Overview page to create a report."
              : "No assessment results for this server. Connect and run an assessment from the Overview page to create a report."}
        </Notice>
      ) : (
        <>
          {incomplete && (
            <Notice tone="warn">
              {incomplete}. A report from this scan is incomplete: steps that did not run are missing from it.
            </Notice>
          )}
          {entry.stale && (
            <Notice tone="warn">
              These results may be out of date: the server is not connected now
              {entry.staleAt ? ` (connection lost ${localTime(entry.staleAt)})` : ""}. The report will say so.
            </Notice>
          )}
          {coverage && <p className="text-sm text-slate-700 dark:text-slate-300">{coverage}</p>}
        </>
      )}

      <section className="flex flex-col gap-3 rounded-lg border border-slate-200 p-4 dark:border-slate-800">
        <label className="flex items-start gap-2 text-sm">
          <input
            type="checkbox"
            className="mt-1"
            checked={redact}
            disabled={busy}
            onChange={(e) => setRedact(e.target.checked)}
          />
          <span>
            <span className="font-medium">Omit connection address</span>
            <span className="block text-xs text-slate-500">
              Removes only the configured connection address. Hostnames, service names, package versions, ports,
              process names and file paths remain. The report is not anonymized. Review it before sharing.
            </span>
          </span>
        </label>

        <div className="flex flex-wrap items-center gap-3 text-sm">
          <span>Preview format:</span>
          {(["json", "html"] as const).map((f) => (
            <label key={f} className="flex items-center gap-1.5">
              <input
                type="radio"
                name="report-format"
                checked={format === f}
                disabled={busy}
                onChange={() => void changeFormat(f)}
              />
              {f === "json" ? "JSON" : "HTML source"}
            </label>
          ))}
          <Button variant="secondary" disabled={!hasScan || busy} onClick={() => void prepare()}>
            {snapshot ? "Refresh report" : "Prepare report"}
          </Button>
        </div>

        {outdated && (
          <Notice tone="warn">
            The assessment or the setting changed after this preview. Refresh the report before exporting.
          </Notice>
        )}
        {feedback && <Notice tone={feedback.tone}>{feedback.text}</Notice>}

        {preview && (
          <>
            <p className="text-xs text-slate-500">
              Preview ({preview.format === "json" ? "JSON" : "HTML source, shown as text"}) of the snapshot frozen
              {snapshot ? ` from scan started ${localTime(snapshot.report.scan.startedAt)}` : ""}
              {snapshot?.redact ? ", connection address omitted" : ", connection address included"}. The saved file
              is built again when you export, so its generation time differs.
            </p>
            <pre className="max-h-96 overflow-auto rounded-md border border-slate-200 bg-slate-50 p-3 text-xs break-words whitespace-pre-wrap dark:border-slate-800 dark:bg-slate-900">
              {preview.text}
            </pre>
          </>
        )}

        <div className="flex flex-wrap gap-2">
          <Button disabled={!canExport} onClick={() => void exportAs("json")}>
            Export JSON
          </Button>
          <Button disabled={!canExport} onClick={() => void exportAs("html")}>
            Export HTML
          </Button>
        </div>
        {!canExport && hasScan && !outdated && (
          <p className="text-xs text-slate-500">Prepare the report to enable export.</p>
        )}
      </section>
    </div>
  );
}
