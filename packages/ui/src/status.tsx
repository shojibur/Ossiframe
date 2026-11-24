import type { ReactNode } from "react";
import { cn } from "./cn";

export type Tone = "ok" | "warn" | "error" | "neutral" | "busy";

const tones: Record<Tone, string> = {
  ok: "bg-emerald-600",
  warn: "bg-amber-500",
  error: "bg-red-600",
  neutral: "bg-slate-400",
  busy: "bg-sky-500 animate-pulse",
};

/** Status is always shown as a marker plus a text label, never color alone. */
export function StatusBadge({ tone, children }: { tone: Tone; children: ReactNode }) {
  return (
    <span className="inline-flex items-center gap-1.5 text-xs text-slate-700 dark:text-slate-300">
      <span aria-hidden className={cn("size-2 rounded-full", tones[tone])} />
      {children}
    </span>
  );
}

export function Notice({ tone, children }: { tone: "error" | "warn" | "info"; children: ReactNode }) {
  const style = {
    error: "border-red-300 bg-red-50 text-red-900 dark:border-red-900 dark:bg-red-950 dark:text-red-200",
    warn: "border-amber-300 bg-amber-50 text-amber-900 dark:border-amber-900 dark:bg-amber-950 dark:text-amber-200",
    info: "border-slate-300 bg-slate-50 text-slate-800 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-200",
  }[tone];
  return (
    <div role={tone === "error" ? "alert" : "status"} className={cn("rounded-md border px-3 py-2 text-sm", style)}>
      {children}
    </div>
  );
}

/** Two-column key/value list. Values render as text only. */
export function KeyValue({ rows }: { rows: [string, ReactNode][] }) {
  return (
    <dl className="grid grid-cols-[minmax(140px,max-content)_1fr] gap-x-6 gap-y-2 text-sm">
      {rows.map(([k, v]) => (
        <div key={k} className="contents">
          <dt className="text-slate-500 dark:text-slate-400">{k}</dt>
          <dd className="text-slate-900 break-words dark:text-slate-100">{v}</dd>
        </div>
      ))}
    </dl>
  );
}

/** Monospace block for fingerprints and paths. */
export function Mono({ children, className }: { children: ReactNode; className?: string }) {
  return <code className={cn("font-mono text-[13px] break-all select-all", className)}>{children}</code>;
}
