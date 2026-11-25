import { Button, Notice, StatusBadge, errorMessage } from "@ossiframe/ui";
import { Section, useComponentList } from "./parts";
import { createPlan, effectiveSelection, setSelection, useServerEntry } from "./store";

export function SoftwarePage({ serverId, connected }: { serverId: string; connected: boolean }) {
  const entry = useServerEntry(serverId);
  const { list, error: listError } = useComponentList();
  if (listError) return <Notice tone="error">{listError}</Notice>;
  if (!list) return <p className="text-sm text-slate-500">Loading components…</p>;

  const selection = effectiveSelection(entry, list);
  const plan = entry.plan?.dto.plan;
  const planCurrent = entry.plan !== null && sameSet(entry.plan.selection, selection);
  const blocked = entry.preflight !== null && !entry.preflight.canProvision;
  const planning = entry.busy === "plan";

  function toggle(id: string, on: boolean) {
    const next = new Set(selection);
    if (on) next.add(id);
    else next.delete(id);
    setSelection(serverId, [...next]);
  }

  function state(id: string) {
    if (!plan || !planCurrent) return <span className="text-xs text-slate-500">Checked when a plan is created</span>;
    if (plan.alreadyPresent.includes(id)) return <StatusBadge tone="ok">Already installed</StatusBadge>;
    if (plan.components.includes(id)) return <StatusBadge tone="warn">Will be installed</StatusBadge>;
    return <StatusBadge tone="neutral">Not selected</StatusBadge>;
  }

  return (
    <div className="flex flex-col gap-5">
      <p className="max-w-3xl text-sm text-slate-600 dark:text-slate-400">
        Choose what to install. Components already on the server are skipped, and more can be installed later by creating a new plan.
        Nothing is installed until you review and approve a plan.
      </p>

      <Section title="Components" aside={`${selection.length} selected`}>
        <ul className="divide-y divide-slate-200 dark:divide-slate-800">
          {list.map((c) => (
            <li key={c.id} className="flex items-start gap-3 py-2">
              <input
                id={`comp-${c.id}`}
                type="checkbox"
                className="mt-1"
                checked={selection.includes(c.id)}
                onChange={(e) => toggle(c.id, e.target.checked)}
              />
              <label htmlFor={`comp-${c.id}`} className="flex min-w-0 flex-1 flex-col gap-0.5">
                <span className="text-sm font-medium">
                  {c.title}
                  {c.optional && <span className="ml-2 text-xs font-normal text-slate-500">optional</span>}
                </span>
                <span className="text-xs text-slate-600 dark:text-slate-400">{c.description}</span>
                {c.requires.length > 0 && (
                  <span className="text-xs text-slate-500">Also needs: {c.requires.join(", ")} (added automatically)</span>
                )}
              </label>
              <div className="shrink-0">{state(c.id)}</div>
            </li>
          ))}
        </ul>
      </Section>

      {blocked && <Notice tone="error">The last preflight found blocking findings. Resolve them on the Preflight page first.</Notice>}
      {entry.error && entry.busy === null && <Notice tone="error">{errorMessage(entry.error)}</Notice>}

      <div className="flex flex-wrap items-center gap-3">
        <Button disabled={!connected || planning || selection.length === 0} onClick={() => void createPlan(serverId, selection)}>
          {planning ? "Creating plan…" : "Create plan"}
        </Button>
        {!connected && <span className="text-sm text-slate-500">Connect to create a plan.</span>}
        {planCurrent && <span className="text-sm text-slate-600 dark:text-slate-400">Plan created. Review it on the Plan page.</span>}
      </div>
    </div>
  );
}

function sameSet(a: string[], b: string[]): boolean {
  return a.length === b.length && a.every((x) => b.includes(x));
}
