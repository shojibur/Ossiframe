import { cn } from "./cn";

export type SeverityLevel = "critical" | "high" | "medium" | "low" | "info";

// Each level has its own symbol and a text label, so color is never the only signal.
const levels: Record<SeverityLevel, { label: string; symbol: string; style: string }> = {
  critical: { label: "Critical", symbol: "■", style: "border-red-700 bg-red-50 text-red-900 dark:bg-red-950 dark:text-red-200" },
  high: { label: "High", symbol: "▲", style: "border-orange-600 bg-orange-50 text-orange-900 dark:bg-orange-950 dark:text-orange-200" },
  medium: { label: "Medium", symbol: "◆", style: "border-amber-500 bg-amber-50 text-amber-900 dark:bg-amber-950 dark:text-amber-200" },
  low: { label: "Low", symbol: "●", style: "border-sky-500 bg-sky-50 text-sky-900 dark:bg-sky-950 dark:text-sky-200" },
  info: { label: "Info", symbol: "○", style: "border-slate-400 bg-slate-50 text-slate-800 dark:bg-slate-900 dark:text-slate-200" },
};

/** Small severity label: symbol plus text. */
export function SeverityBadge({ severity }: { severity: SeverityLevel }) {
  const l = levels[severity];
  return (
    <span className={cn("inline-flex items-center gap-1 rounded border px-1.5 py-0.5 text-xs font-medium", l.style)}>
      <span aria-hidden>{l.symbol}</span>
      {l.label}
    </span>
  );
}
