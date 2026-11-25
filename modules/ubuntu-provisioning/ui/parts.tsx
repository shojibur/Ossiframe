import { useEffect, type ReactNode } from "react";
import type { ComponentInfo, OperationState, ServerClass, SudoMode } from "@ossiframe/client";
import { StatusBadge, type Tone } from "@ossiframe/ui";
import { loadComponents, useComponents } from "./store";

/** Loads the component list once and returns it. Remote text is never interpreted. */
export function useComponentList(): { list: ComponentInfo[] | null; error: string | null } {
  const c = useComponents();
  useEffect(() => {
    if (!c.list && !c.error) void loadComponents();
  }, [c.list, c.error]);
  return c;
}

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

export function Table({ head, children }: { head: string[]; children: ReactNode }) {
  return (
    <div className="overflow-x-auto rounded-md border border-slate-200 dark:border-slate-800">
      <table className="w-full text-left text-sm">
        <thead className="bg-slate-50 text-xs text-slate-500 uppercase dark:bg-slate-900 dark:text-slate-400">
          <tr>
            {head.map((h) => (
              <th key={h} className="px-3 py-1.5 font-medium">
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

export const Td = ({ children, mono }: { children: ReactNode; mono?: boolean }) => (
  <td className={`px-3 py-1.5 align-top break-words ${mono ? "font-mono text-[13px]" : ""}`}>{children}</td>
);

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

export const CLASS_LABEL: Record<ServerClass, string> = {
  fresh: "Fresh server",
  existingUnmanaged: "Existing server not managed by Ossiframe",
  ossiframeManaged: "Server provisioned by Ossiframe",
  partial: "Partially provisioned (earlier operation did not finish)",
};

export const SUDO_LABEL: Record<SudoMode, string> = {
  passwordless: "Passwordless sudo available",
  passwordRequired: "sudo needs a password (not supported)",
  notPermitted: "This user is not permitted to use sudo",
  unavailable: "sudo is not available",
};

const STATE: Record<OperationState, [Tone, string]> = {
  pending: ["neutral", "Pending"],
  running: ["busy", "Running"],
  verifying: ["busy", "Verifying"],
  succeeded: ["ok", "Succeeded"],
  failed: ["error", "Failed"],
  cancelled: ["neutral", "Cancelled"],
  unknownOutcome: ["warn", "Unknown outcome"],
};

export function OperationStateBadge({ state }: { state: OperationState }) {
  const [tone, label] = STATE[state];
  return <StatusBadge tone={tone}>{label}</StatusBadge>;
}

export function mb(n: number | null): string {
  if (n === null) return "unknown";
  return n >= 1024 ? `${(n / 1024).toFixed(1)} GiB` : `${n} MiB`;
}
