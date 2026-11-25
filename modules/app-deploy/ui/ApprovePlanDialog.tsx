import { useState, type ReactNode } from "react";
import { appsApi, toIpcError, type PlanDto } from "@ossiframe/client";
import { Button, Dialog, KeyValue, Mono, Notice, errorMessage } from "@ossiframe/ui";
import { Bullets } from "./parts";
import { setView } from "./store";

export interface PlanSummary {
  /** What the plan is for, shown at the top, for example "Create application". */
  action: string;
  slug: string;
  appId: string | null;
  /** Rows describing the application (type, domain, repository). */
  rows: [string, string][];
  /** Things the plan creates that the plan's file list does not name. */
  creates: string[];
  /** The plan creates a database whose password only the server keeps. */
  database: boolean;
  /** Go back to the application list when the operation succeeds (for example after removal). */
  returnToList?: boolean;
}

/**
 * Final review before anything changes on the server. Everything shown comes from the
 * plan the backend returned. Approving sends only the server id, plan id and digest.
 */
export function ApprovePlanDialog({
  serverId,
  dto,
  summary,
  connected,
  onClose,
}: {
  serverId: string;
  dto: PlanDto;
  summary: PlanSummary;
  connected: boolean;
  onClose: () => void;
}) {
  const { plan } = dto;
  const t = plan.target;
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function approve() {
    setBusy(true);
    setError(null);
    try {
      const started = await appsApi.approvePlan(serverId, dto.planId, dto.digest);
      setView(serverId, {
        kind: "operation",
        operationId: started.operationId,
        slug: summary.slug,
        appId: summary.appId ?? t.appId ?? null,
        returnToList: summary.returnToList === true,
      });
      onClose();
    } catch (e) {
      setError(errorMessage(toIpcError(e)));
      setBusy(false);
    }
  }

  const creates = [...summary.creates, ...plan.files.map((f) => `${f.path}: ${f.description}`)];
  const minutes = Math.max(1, Math.round(dto.expiresInSecs / 60));
  return (
    <Dialog
      open
      onOpenChange={(o) => !o && !busy && onClose()}
      title={`${summary.action}: review and approve`}
      description="Nothing has been changed yet. Approving starts the changes on the server."
      className="w-[min(720px,94vw)]"
      footer={
        <>
          <Button variant="secondary" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button onClick={() => void approve()} disabled={busy || !connected}>
            {busy ? "Starting…" : "Approve and start"}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4 text-sm">
        {error && <Notice tone="error">{error}</Notice>}
        {!connected && <Notice tone="warn">The server is not connected. Connect it to approve.</Notice>}
        {dto.warnings.map((w) => (
          <Notice key={w.id} tone="warn">
            {w.message}
          </Notice>
        ))}
        <KeyValue
          rows={[
            ["Target server", `${t.name} (${t.user}@${t.host}:${t.port})`],
            ["Application", summary.slug],
            ...summary.rows,
          ]}
        />
        <Block title={`Steps (${plan.steps.length})`}>
          <ol className="list-decimal space-y-1 pl-5">
            {plan.steps.map((s) => (
              <li key={s.id}>{s.title}</li>
            ))}
          </ol>
        </Block>
        <Block title="Commands that run as root">
          {plan.privileges.length === 0 ? (
            <p className="text-slate-600 dark:text-slate-400">No command runs as root.</p>
          ) : (
            <Bullets items={plan.privileges} />
          )}
          <p className="text-slate-600 dark:text-slate-400">
            Everything inside the application's folder runs as its own Linux user, not as root.
          </p>
        </Block>
        {creates.length > 0 && (
          <Block title="What is created or written">
            <Bullets items={creates} />
          </Block>
        )}
        {plan.services.length > 0 && (
          <Block title="Services affected">
            <Bullets items={plan.services.map((s) => `${s.unit}: ${s.effect}`)} />
          </Block>
        )}
        <Block title="If something goes wrong">
          <ul className="list-disc space-y-1 pl-5">
            <li>There is no automatic rollback. A failed step stops the operation and leaves what already happened in place.</li>
            <li>If Ossiframe loses the connection, the outcome is shown as unknown. Nothing is retried automatically.</li>
            {summary.database && (
              <li>
                The database password is generated on the server and stored only in the application's <Mono>shared/.env</Mono>. It
                is never shown here or saved by Ossiframe.
              </li>
            )}
            {plan.limitations.map((l, i) => (
              <li key={i}>{l}</li>
            ))}
          </ul>
        </Block>
        <p className="text-xs text-slate-500">
          This approval applies to exactly this plan and expires in about {minutes} {minutes === 1 ? "minute" : "minutes"}.
        </p>
        <details className="text-xs">
          <summary className="cursor-pointer text-slate-600 dark:text-slate-400">Technical details</summary>
          <div className="mt-2 flex flex-col gap-1">
            <span className="text-slate-500">Pinned server host key (SHA-256)</span>
            <Mono>{t.hostKeySha256}</Mono>
            <span className="text-slate-500">Plan digest (SHA-256)</span>
            <Mono>{dto.digest}</Mono>
            <span className="text-slate-500">Plan id</span>
            <Mono>{dto.planId}</Mono>
          </div>
        </details>
      </div>
    </Dialog>
  );
}

function Block({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div>
      <h3 className="mb-1 text-xs font-semibold text-slate-500 uppercase">{title}</h3>
      {children}
    </div>
  );
}
