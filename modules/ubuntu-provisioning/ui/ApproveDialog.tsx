import { Button, Dialog, KeyValue, Mono } from "@ossiframe/ui";
import type { PlanDto } from "@ossiframe/client";
import { Bullets, mb } from "./parts";

/**
 * Final review before anything changes on the server. Every value shown here comes from
 * the plan the backend returned. Approving sends only the plan id and digest.
 */
export function ApproveDialog({
  dto,
  busy,
  onCancel,
  onApprove,
}: {
  dto: PlanDto;
  busy: boolean;
  onCancel: () => void;
  onApprove: () => void;
}) {
  const { plan } = dto;
  const t = plan.target;
  const toInstall = plan.packages.map((p) => p.name);
  return (
    <Dialog
      open
      onOpenChange={(o) => !o && !busy && onCancel()}
      title="Approve this plan"
      description="Approving starts changes on the server. Review each part first."
      className="w-[min(680px,94vw)]"
      footer={
        <>
          <Button variant="secondary" onClick={onCancel} disabled={busy}>
            Cancel
          </Button>
          <Button onClick={onApprove} disabled={busy}>
            {busy ? "Starting…" : "Approve and start"}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4 text-sm">
        <KeyValue
          rows={[
            ["Target server", `${t.name} (${t.user}@${t.host}:${t.port})`],
            ["Pinned host key (SHA-256)", <Mono key="hk">{t.hostKeySha256}</Mono>],
            ["Operating system", plan.os],
            ["Selected components", plan.components.join(", ") || "none"],
            ["Already installed", plan.alreadyPresent.join(", ") || "none"],
          ]}
        />

        <div>
          <h3 className="mb-1 text-xs font-semibold text-slate-500 uppercase">Packages to install ({toInstall.length})</h3>
          {toInstall.length ? (
            <p className="font-mono text-[13px] break-words">{toInstall.join(" ")}</p>
          ) : (
            <p className="text-slate-600 dark:text-slate-400">None. The operation would only verify what is installed.</p>
          )}
        </div>

        <div>
          <h3 className="mb-1 text-xs font-semibold text-slate-500 uppercase">Expected service changes</h3>
          {plan.services.length ? (
            <Bullets items={plan.services.map((s) => `${s.unit}: ${s.effect}`)} />
          ) : (
            <p className="text-slate-600 dark:text-slate-400">None listed.</p>
          )}
        </div>

        <div>
          <h3 className="mb-1 text-xs font-semibold text-slate-500 uppercase">Privileges</h3>
          <p>
            Runs as root through <span className="font-mono">sudo -n</span>. {plan.privileges.join(" ")}
          </p>
        </div>

        <div>
          <h3 className="mb-1 text-xs font-semibold text-slate-500 uppercase">Resources</h3>
          <p>
            About {mb(plan.resources.diskMbNeeded)} of disk, at least {mb(plan.resources.memMbMin)} of memory.
          </p>
        </div>

        <div>
          <h3 className="mb-1 text-xs font-semibold text-slate-500 uppercase">Recovery limits</h3>
          <Bullets items={plan.limitations} />
          <ul className="mt-1 list-disc space-y-1 pl-5">
            <li>APT may install additional dependency packages beyond the list above and runs the packages&apos; maintainer scripts as root.</li>
            <li>There is no automatic rollback. If it fails or the outcome is unknown, you inspect the server and decide.</li>
          </ul>
        </div>

        <p className="text-xs text-slate-500">
          The plan digest ties your approval to the exact plan the backend created. It does not protect against a compromised
          frontend. The digest is shown under technical details on the Plan page.
        </p>
      </div>
    </Dialog>
  );
}
