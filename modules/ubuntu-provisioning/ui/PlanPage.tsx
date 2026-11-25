import { useEffect, useState } from "react";
import { Button, Dialog, KeyValue, Mono, Notice, errorMessage } from "@ossiframe/ui";
import { ApproveDialog } from "./ApproveDialog";
import { Bullets, Section, Table, Td, mb } from "./parts";
import { approvePlan, clearError, discardPlan, releaseStaleLock, useServerEntry } from "./store";

function useNow(intervalMs: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(t);
  }, [intervalMs]);
  return now;
}

function remaining(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

export function PlanPage({ serverId, connected }: { serverId: string; connected: boolean }) {
  const entry = useServerEntry(serverId);
  const now = useNow(1000);
  const [reviewing, setReviewing] = useState(false);
  const [releasing, setReleasing] = useState(false);
  const stored = entry.plan;
  const busyApprove = entry.busy === "approve";
  const err = entry.error;
  const lockBusy = err?.kind === "plan" && err.reason === "busy";

  if (!stored) {
    return (
      <div className="flex flex-col gap-4">
        {err && <Notice tone="error">{errorMessage(err)}</Notice>}
        {lockBusy && <StaleLock onRelease={() => setReleasing(true)} />}
        <p className="text-sm text-slate-500">
          There is no plan to review. Choose what to add on the Software page and select Review install plan. A plan expires after a
          few minutes and can be approved once.
        </p>
        {releasing && <ReleaseDialog serverId={serverId} onClose={() => setReleasing(false)} />}
      </div>
    );
  }

  const { dto } = stored;
  const { plan } = dto;
  const left = dto.expiresInSecs - (now - stored.receivedAt) / 1000;
  const expired = left <= 0;

  return (
    <div className="flex flex-col gap-5">
      {err && <Notice tone="error">{errorMessage(err)}</Notice>}
      {lockBusy && <StaleLock onRelease={() => setReleasing(true)} />}
      {expired ? (
        <Notice tone="warn">This plan expired and nothing was changed. Create a new one on the Software page.</Notice>
      ) : (
        <Notice tone="info">
          Nothing has been changed yet. This plan expires in <span className="tabular-nums">{remaining(left)}</span>.
        </Notice>
      )}
      {dto.warnings.map((w) => (
        <Notice key={w.id} tone="warn">
          {w.message}
        </Notice>
      ))}

      <Section title="Target">
        <KeyValue
          rows={[
            ["Server", `${plan.target.name} (${plan.target.user}@${plan.target.host}:${plan.target.port})`],
            ["Operating system", plan.os],
            ["Components", plan.components.join(", ") || "none"],
            ["Already installed", plan.alreadyPresent.join(", ") || "none"],
          ]}
        />
      </Section>

      <Section title={`Packages to install (${plan.packages.length})`}>
        {plan.packages.length === 0 ? (
          <p className="text-sm text-slate-500">Nothing to install. Approving only checks that the selected components work.</p>
        ) : (
          <>
            <p className="font-mono text-[13px] break-words">{plan.packages.map((p) => p.name).join(" ")}</p>
            <p className="text-xs text-slate-500">APT may install additional dependencies that are not listed here.</p>
          </>
        )}
      </Section>

      <Section title="Expected service changes">
        {plan.services.length === 0 ? (
          <p className="text-sm text-slate-500">None listed.</p>
        ) : (
          <Table head={["Service", "Effect"]}>
            {plan.services.map((s) => (
              <tr key={s.unit}>
                <Td mono>{s.unit}</Td>
                <Td>{s.effect}</Td>
              </tr>
            ))}
          </Table>
        )}
      </Section>

      <Section title="Steps">
        {plan.steps.length === 0 ? (
          <p className="text-sm text-slate-500">No steps. Verification only.</p>
        ) : (
          <ol className="list-decimal space-y-1 pl-5 text-sm">
            {plan.steps.map((s) => (
              <li key={s.id}>{s.title}</li>
            ))}
          </ol>
        )}
      </Section>

      <Section title="Privileges and resources">
        {plan.privileges.length === 0 ? (
          <p className="text-sm text-slate-500">No command runs as root.</p>
        ) : (
          <Bullets items={plan.privileges} />
        )}
        <p className="text-sm">
          About {mb(plan.resources.diskMbNeeded)} of disk and at least {mb(plan.resources.memMbMin)} of memory.
        </p>
      </Section>

      <Section title="Limitations">
        {plan.limitations.length === 0 ? (
          <p className="text-sm text-slate-500">Nothing is installed, so there is nothing to roll back.</p>
        ) : (
          <Bullets items={plan.limitations} />
        )}
      </Section>

      <details className="text-sm">
        <summary className="cursor-pointer text-slate-600 dark:text-slate-400">Technical details</summary>
        <div className="mt-2 flex flex-col gap-1">
          <span className="text-xs text-slate-500">Plan digest (SHA-256)</span>
          <Mono>{dto.digest}</Mono>
          <span className="text-xs text-slate-500">Plan id</span>
          <Mono>{dto.planId}</Mono>
        </div>
      </details>

      <div className="flex flex-wrap gap-2">
        <Button disabled={!connected || expired || busyApprove} onClick={() => setReviewing(true)}>
          Review and approve…
        </Button>
        <Button variant="secondary" disabled={busyApprove} onClick={() => discardPlan(serverId)}>
          Discard plan
        </Button>
        {!connected && <span className="self-center text-sm text-slate-500">Connect to approve.</span>}
      </div>

      {reviewing && !expired && (
        <ApproveDialog
          dto={dto}
          busy={busyApprove}
          onCancel={() => {
            setReviewing(false);
            clearError(serverId);
          }}
          onApprove={() => void approvePlan(serverId).then((ok) => ok && setReviewing(false))}
        />
      )}
      {releasing && <ReleaseDialog serverId={serverId} onClose={() => setReleasing(false)} />}
    </div>
  );
}

function StaleLock({ onRelease }: { onRelease: () => void }) {
  return (
    <div className="flex flex-wrap items-center gap-3">
      <span className="text-sm text-slate-600 dark:text-slate-400">
        If the holder above is an Ossiframe operation that no longer exists, the lock may be stale.
      </span>
      <Button size="sm" variant="secondary" onClick={onRelease}>
        Release stale lock…
      </Button>
    </div>
  );
}

function ReleaseDialog({ serverId, onClose }: { serverId: string; onClose: () => void }) {
  const [busy, setBusy] = useState(false);
  return (
    <Dialog
      open
      onOpenChange={(o) => !o && !busy && onClose()}
      title="Release stale lock"
      description="Only release the lock after you checked that no other Ossiframe installation is using this server."
      footer={
        <>
          <Button variant="secondary" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            disabled={busy}
            onClick={() => {
              setBusy(true);
              void releaseStaleLock(serverId).then((ok) => {
                setBusy(false);
                if (ok) onClose();
              });
            }}
          >
            Release lock
          </Button>
        </>
      }
    >
      <p className="text-sm">
        The app checks the server first and refuses to release the lock while an Ossiframe step unit, an apt or dpkg process or a
        held package lock file exists. The decision and the evidence are recorded.
      </p>
    </Dialog>
  );
}
