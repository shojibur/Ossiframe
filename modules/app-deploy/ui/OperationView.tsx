import { useCallback, useEffect, useState } from "react";
import {
  appsApi,
  onOperationUpdated,
  provisioningApi,
  toIpcError,
  type AppSummaryDto,
  type OperationDetailDto,
} from "@ossiframe/client";
import { Button, Notice, errorMessage } from "@ossiframe/ui";
import { OperationBadge, Section } from "./parts";
import { setView } from "./store";

const POLL_MS = 3000;

/** Progress of a started operation. Polling is the source of truth; the event only makes it faster. */
export function OperationView({
  serverId,
  operationId,
  slug,
  appId,
  returnToList = false,
}: {
  serverId: string;
  operationId: string;
  slug: string;
  appId: string | null;
  returnToList?: boolean;
}) {
  const [op, setOp] = useState<OperationDetailDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [app, setApp] = useState<AppSummaryDto | null>(null);
  const [cancelling, setCancelling] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setOp(await provisioningApi.getOperation(operationId));
      setError(null);
    } catch (e) {
      setError(errorMessage(toIpcError(e)));
    }
  }, [operationId]);

  const state = op?.state;
  const active = state === undefined || state === "pending" || state === "running" || state === "verifying";

  useEffect(() => {
    provisioningApi.getOperation(operationId).then(setOp, (e: unknown) => setError(errorMessage(toIpcError(e))));
    let unlisten: (() => void) | null = null;
    let gone = false;
    onOperationUpdated((e) => e.operationId === operationId && void refresh()).then(
      (u) => (gone ? u() : (unlisten = u)),
      () => undefined, // No Tauri runtime. Polling still works.
    );
    return () => {
      gone = true;
      unlisten?.();
    };
  }, [operationId, refresh]);

  useEffect(() => {
    if (!active) return;
    const t = setInterval(() => void refresh(), POLL_MS);
    return () => clearInterval(t);
  }, [active, refresh]);

  // When the operation ends, find the application so the user can continue with it.
  useEffect(() => {
    if (active) return;
    appsApi.list(serverId).then(
      (list) => setApp(list.find((a) => a.id === appId || a.slug === slug) ?? null),
      () => undefined,
    );
  }, [active, serverId, appId, slug]);

  // A finished removal has nothing left to open, so go back to the list.
  useEffect(() => {
    if (returnToList && state === "succeeded") setView(serverId, { kind: "list" });
  }, [returnToList, state, serverId]);

  async function cancel() {
    setCancelling(true);
    try {
      await provisioningApi.cancel(operationId);
      await refresh();
    } catch (e) {
      setError(errorMessage(toIpcError(e)));
    }
    setCancelling(false);
  }

  const back = () => setView(serverId, { kind: "list" });
  const open = () => app && setView(serverId, { kind: "detail", appId: app.id });

  return (
    <div className="flex max-w-3xl flex-col gap-5">
      <h2 className="text-base font-semibold">Progress: {slug}</h2>
      {error && <Notice tone="error">{error}</Notice>}
      {!op ? (
        <p className="text-sm text-slate-500">Loading…</p>
      ) : (
        <>
          <Section title="Status" aside={<OperationBadge state={op.state} />}>
            {op.error && <Notice tone="error">{op.error}</Notice>}
            {active && (
              <div className="flex flex-wrap items-center gap-3">
                <span className="text-sm text-slate-600 dark:text-slate-400">
                  This can take a few minutes. You can switch tabs; the operation keeps running.
                </span>
                {op.cancelRequested ? (
                  <span className="text-sm text-amber-700 dark:text-amber-400">Cancel requested. Waiting for the current step.</span>
                ) : (
                  <Button variant="secondary" size="sm" disabled={cancelling} onClick={() => void cancel()}>
                    Cancel after current step
                  </Button>
                )}
              </div>
            )}
            {op.state === "succeeded" && <Notice tone="info">Done. The server was changed as approved and verified.</Notice>}
            {op.state === "failed" && (
              <Notice tone="warn">
                The operation failed. There is no automatic rollback, so what already happened stays in place. Check the failed step
                below.
              </Notice>
            )}
            {op.state === "unknownOutcome" && (
              <Notice tone="warn">
                Ossiframe lost track of this operation and did not retry it. Open the Operation tab to reconcile or close it after
                you inspect the server.
              </Notice>
            )}
            {!active && (
              <div className="flex flex-wrap gap-2">
                {app && (
                  <Button onClick={open}>
                    {app.state === "awaiting-deploy" ? "Continue: deploy setup" : "Open application"}
                  </Button>
                )}
                <Button variant="secondary" onClick={back}>
                  Back to applications
                </Button>
              </div>
            )}
          </Section>
          <Section title="Steps">
            <ol className="flex flex-col gap-3">
              {op.steps.map((s) => (
                <li key={s.idx} className="flex flex-col gap-1">
                  <div className="flex flex-wrap items-center justify-between gap-2 text-sm">
                    <span className="font-medium">
                      {s.idx + 1}. {s.title}
                    </span>
                    <span className="text-xs text-slate-600 dark:text-slate-400">{s.status}</span>
                  </div>
                  {s.output && (s.status === "failed" || s.exitCode !== 0) && (
                    <pre className="max-h-48 overflow-auto rounded-md border border-slate-200 bg-slate-50 p-2 font-mono text-xs break-words whitespace-pre-wrap dark:border-slate-800 dark:bg-slate-900">
                      {s.output}
                    </pre>
                  )}
                </li>
              ))}
              {op.steps.length === 0 && <li className="text-sm text-slate-500">No steps recorded yet.</li>}
            </ol>
            <p className="text-xs text-slate-500">Output is shown for steps that did not succeed. It is bounded and redacted.</p>
          </Section>
        </>
      )}
    </div>
  );
}
