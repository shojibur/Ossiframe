import { useEffect, useState } from "react";
import { provisioningApi, toIpcError, type EvidenceReport, type OperationDetailDto, type VerificationDoc } from "@ossiframe/client";
import { Button, Dialog, KeyValue, Notice, StatusBadge, errorMessage, localTime, type Tone } from "@ossiframe/ui";
import { OperationStateBadge, Section, Table, Td } from "./parts";
import { cancelOperation, ensureOperation, refreshOperation, useServerEntry } from "./store";

const POLL_MS = 3000;

export function OperationPage({ serverId, connected }: { serverId: string; connected: boolean }) {
  const entry = useServerEntry(serverId);
  const op = entry.operation;
  const id = entry.operationId;
  const state = op?.state;
  const polling = state === "pending" || state === "running" || state === "verifying";

  useEffect(() => {
    void ensureOperation(serverId);
  }, [serverId]);

  // Polling is the source of truth. The operation-updated event only makes it faster.
  useEffect(() => {
    if (!id || !polling) return;
    const t = setInterval(() => void refreshOperation(id), POLL_MS);
    return () => clearInterval(t);
  }, [id, polling]);

  if (!op) {
    return (
      <div className="flex flex-col gap-3">
        {entry.error && <Notice tone="error">{errorMessage(entry.error)}</Notice>}
        <p className="text-sm text-slate-500">{id ? "Loading operation…" : "No operation yet. After you approve a plan, its progress appears here."}</p>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-5">
      {entry.error && <Notice tone="error">{errorMessage(entry.error)}</Notice>}

      <Section title="Operation" aside={<OperationStateBadge state={op.state} />}>
        <KeyValue
          rows={[
            ["Server", `${op.server.name} (${op.server.user}@${op.server.host}:${op.server.port})`],
            ["Created", localTime(op.createdAt)],
            ["Started", op.startedAt ? localTime(op.startedAt) : "not started"],
            ["Finished", op.finishedAt ? localTime(op.finishedAt) : "not finished"],
            ["Components", op.plan?.components.join(", ") ?? "unknown"],
          ]}
        />
        {op.error && <Notice tone="error">{op.error}</Notice>}
        {polling && (
          <div className="flex flex-wrap items-center gap-3">
            {op.cancelRequested ? (
              <span className="text-sm text-amber-700 dark:text-amber-400">
                Cancel requested, waiting for the current step to finish. A running package step is not interrupted.
              </span>
            ) : (
              <Button variant="secondary" onClick={() => void cancelOperation(serverId, op.id)}>
                Cancel after current step
              </Button>
            )}
          </div>
        )}
      </Section>

      {op.state === "unknownOutcome" && <UnknownOutcome op={op} connected={connected} />}

      <Section title="Steps">
        {op.steps.length === 0 ? (
          <p className="text-sm text-slate-500">No install steps. This operation only checked the components.</p>
        ) : (
          <ol className="flex flex-col gap-3">
            {op.steps.map((s) => (
              <li key={s.idx} className="flex flex-col gap-1">
                <div className="flex flex-wrap items-center justify-between gap-2 text-sm">
                  <span className="font-medium">
                    {s.idx + 1}. {s.title}
                  </span>
                  <span className="text-xs text-slate-600 dark:text-slate-400">
                    {s.status}
                    {s.exitCode !== null ? `, exit ${s.exitCode}` : ""}
                  </span>
                </div>
                {s.output ? (
                  <pre className="max-h-64 overflow-auto rounded-md border border-slate-200 bg-slate-50 p-2 font-mono text-xs break-words whitespace-pre-wrap dark:border-slate-800 dark:bg-slate-900">
                    {s.output}
                  </pre>
                ) : (
                  <p className="text-xs text-slate-500">No output recorded yet.</p>
                )}
              </li>
            ))}
          </ol>
        )}
        <p className="text-xs text-slate-500">Logs are bounded, redacted best effort and shown as plain text. They are not a full transcript.</p>
      </Section>

      {op.verification && <Verification doc={op.verification} />}
      {!op.verification && (op.state === "verifying" || op.state === "running") && (
        <p className="text-sm text-slate-500">Verification runs after all steps finish.</p>
      )}

      {op.resolution && (
        <Section title="Closed by you after inspection" aside={localTime(op.resolution.decidedAt)}>
          <p className="text-sm">{op.resolution.decision}</p>
          <Evidence report={op.resolution.evidence} />
        </Section>
      )}
    </div>
  );
}

const CHECK_TONE: Record<string, Tone> = { pass: "ok", fail: "error", warn: "warn", skipped: "neutral" };

function Verification({ doc }: { doc: VerificationDoc }) {
  const components = [...new Set([...doc.verdicts.map((v) => v.component), ...doc.checks.map((c) => c.component)])];
  return (
    <Section title="Verification">
      {components.map((comp) => {
        const verified = doc.verdicts.find((v) => v.component === comp)?.verified === true;
        const checks = doc.checks.filter((c) => c.component === comp);
        return (
          <div key={comp} className="flex flex-col gap-2">
            <div className="flex items-center justify-between text-sm">
              <span className="font-medium">{comp}</span>
              <StatusBadge tone={verified ? "ok" : "error"}>{verified ? "Verified" : "Not verified"}</StatusBadge>
            </div>
            <Table head={["Check", "Kind", "Result", "Detail"]}>
              {checks.map((c) => (
                <tr key={c.id}>
                  <Td>{c.title}</Td>
                  <Td>{c.essential ? "Essential" : "Optional"}</Td>
                  <Td>
                    <StatusBadge tone={CHECK_TONE[c.status] ?? "neutral"}>{c.status}</StatusBadge>
                  </Td>
                  <Td>{c.detail}</Td>
                </tr>
              ))}
            </Table>
          </div>
        );
      })}
      <p className="text-xs text-slate-500">
        A component counts as verified only when every essential check passes. Optional checks only add information.
      </p>
    </Section>
  );
}

function Evidence({ report }: { report: EvidenceReport }) {
  if (report.items.length === 0) return <p className="text-sm text-slate-500">No evidence was collected.</p>;
  return (
    <dl className="grid grid-cols-[minmax(140px,max-content)_1fr] gap-x-6 gap-y-1 text-sm">
      {report.items.map((i, n) => (
        <div key={n} className="contents">
          <dt className="text-slate-500">{i.label}</dt>
          <dd className="font-mono text-[13px] break-words">{i.value}</dd>
        </div>
      ))}
    </dl>
  );
}

function UnknownOutcome({ op, connected }: { op: OperationDetailDto; connected: boolean }) {
  const [busy, setBusy] = useState<"reconcile" | "resolve" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [summary, setSummary] = useState<{ text: string; evidence: EvidenceReport } | null>(null);
  const [confirm, setConfirm] = useState(false);

  async function reconcile() {
    setBusy("reconcile");
    setError(null);
    try {
      const r = await provisioningApi.reconcile(op.id);
      setSummary({ text: r.summary, evidence: r.evidence });
      await refreshOperation(op.id);
    } catch (e) {
      setError(errorMessage(toIpcError(e)));
    }
    setBusy(null);
  }

  async function resolve() {
    setBusy("resolve");
    setError(null);
    try {
      const r = await provisioningApi.markResolved(op.id);
      setSummary({ text: "The operation was closed as failed after your inspection. Nothing was retried or repaired.", evidence: r.evidence });
      setConfirm(false);
      await refreshOperation(op.id);
    } catch (e) {
      setConfirm(false);
      setError(errorMessage(toIpcError(e)));
    }
    setBusy(null);
  }

  return (
    <Section title="Outcome unknown">
      <Notice tone="warn">
        Ossiframe lost track of this operation, so a package command may still be running or may have stopped part way. Nothing is
        retried automatically. Reconcile reads the server and reports what it can prove. If it cannot decide, inspect the server
        yourself (for example <span className="font-mono">dpkg --audit</span> and running apt processes), then close the operation.
      </Notice>
      {error && <Notice tone="error">{error}</Notice>}
      <div className="flex flex-wrap gap-2">
        <Button disabled={!connected || busy !== null} onClick={() => void reconcile()}>
          {busy === "reconcile" ? "Reconciling…" : "Reconcile"}
        </Button>
        <Button variant="secondary" disabled={!connected || busy !== null} onClick={() => setConfirm(true)}>
          Mark resolved after inspection…
        </Button>
        {!connected && <span className="self-center text-sm text-slate-500">Connect to the server to reconcile or close.</span>}
      </div>
      {summary && (
        <div className="flex flex-col gap-2">
          <p className="text-sm">{summary.text}</p>
          <Evidence report={summary.evidence} />
        </div>
      )}
      {confirm && (
        <Dialog
          open
          onOpenChange={(o) => !o && busy === null && setConfirm(false)}
          title="Mark resolved after inspection"
          description="This closes the operation as failed so the server can be used again. It does not repair or retry anything."
          footer={
            <>
              <Button variant="secondary" disabled={busy !== null} onClick={() => setConfirm(false)}>
                Cancel
              </Button>
              <Button variant="destructive" disabled={busy !== null} onClick={() => void resolve()}>
                {busy === "resolve" ? "Checking…" : "Mark resolved"}
              </Button>
            </>
          }
        >
          <div className="flex flex-col gap-2 text-sm">
            <p>
              Server: {op.server.name} ({op.server.user}@{op.server.host}:{op.server.port})
            </p>
            <p>
              The app collects its own fresh evidence first. It refuses if an apt or dpkg process is running, a package lock is held
              or an Ossiframe step unit is still active. Your decision and the evidence are recorded in the operation history.
            </p>
          </div>
        </Dialog>
      )}
    </Section>
  );
}
