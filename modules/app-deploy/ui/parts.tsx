import type { ReactNode } from "react";
import type { OperationState } from "@ossiframe/client";
import { StatusBadge, type Tone } from "@ossiframe/ui";

export function Section({ title, aside, children }: { title: string; aside?: ReactNode; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-3 rounded-lg border border-slate-200 p-4 dark:border-slate-800">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 className="text-sm font-semibold">{title}</h2>
        {aside && <span className="text-xs text-slate-500">{aside}</span>}
      </div>
      {children}
    </div>
  );
}

export function Bullets({ items }: { items: string[] }) {
  if (items.length === 0) return <p className="text-sm text-slate-500">None.</p>;
  return (
    <ul className="list-disc space-y-1 pl-5 text-sm">
      {items.map((t, i) => (
        <li key={i}>{t}</li>
      ))}
    </ul>
  );
}

const APP_STATE: Record<string, [Tone, string]> = {
  provisioning: ["busy", "Setting up"],
  active: ["ok", "Active"],
  deploying: ["busy", "Deploying"],
  "awaiting-deploy": ["warn", "Waiting for first deploy"],
  failed: ["error", "Needs attention"],
  removed: ["neutral", "Removed"],
};

export function AppStateBadge({ state }: { state: string }) {
  const [tone, label] = APP_STATE[state] ?? ["neutral", state];
  return <StatusBadge tone={tone}>{label}</StatusBadge>;
}

const OP_STATE: Record<OperationState, [Tone, string]> = {
  pending: ["neutral", "Waiting to start"],
  running: ["busy", "Running"],
  verifying: ["busy", "Verifying"],
  succeeded: ["ok", "Succeeded"],
  failed: ["error", "Failed"],
  cancelled: ["neutral", "Cancelled"],
  unknownOutcome: ["warn", "Outcome unknown"],
};

export function OperationBadge({ state }: { state: OperationState }) {
  const [tone, label] = OP_STATE[state];
  return <StatusBadge tone={tone}>{label}</StatusBadge>;
}

export const TYPE_LABEL: Record<string, string> = { laravel: "Laravel", php: "PHP", static: "Static" };

/** Release ids look like `20250131T235959Z` (UTC). Returns local time, or the id if it has another shape. */
export function releaseTime(id: string | null | undefined): string {
  if (!id) return "Not deployed yet";
  const m = /^(\d{4})(\d{2})(\d{2})T(\d{2})(\d{2})(\d{2})Z$/.exec(id);
  if (!m) return id;
  const [, y, mo, d, h, mi, s] = m.map(Number);
  const t = new Date(Date.UTC(y ?? 0, (mo ?? 1) - 1, d ?? 1, h ?? 0, mi ?? 0, s ?? 0));
  return Number.isNaN(t.getTime()) ? id : t.toLocaleString();
}
