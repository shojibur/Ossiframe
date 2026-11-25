import type { ComponentInfo } from "@ossiframe/client";
import { Button, Notice, StatusBadge, errorMessage, localTime } from "@ossiframe/ui";
import { Section, useComponentList } from "./parts";
import {
  createPlan,
  effectiveSelection,
  installState,
  preflightSelection,
  runPreflight,
  setSelection,
  useServerEntry,
  type InstallState,
} from "./store";

export function SoftwarePage({ serverId, connected }: { serverId: string; connected: boolean }) {
  const entry = useServerEntry(serverId);
  const { list, error: listError } = useComponentList();
  if (listError) return <Notice tone="error">{listError}</Notice>;
  if (!list) return <p className="text-sm text-slate-500">Loading components…</p>;

  const preflight = entry.preflight;
  const selection = effectiveSelection(entry, list);
  const planCurrent = entry.plan !== null && sameSet(entry.plan.selection, selection);
  const blockers = preflight?.report.conflicts.filter((c) => c.severity === "blocker") ?? [];
  const blocked = preflight !== null && !preflight.canProvision;
  const checking = entry.busy === "preflight";
  const planning = entry.busy === "plan";
  // A plan refused because of blockers says the same as the list of blockers below.
  const err = entry.error;
  const showError = err !== null && entry.busy === null && !(blocked && err.kind === "plan" && err.reason === "blocked");

  function toggle(id: string, on: boolean) {
    const next = new Set(selection);
    if (on) next.add(id);
    else next.delete(id);
    setSelection(serverId, [...next]);
  }

  const check = (
    <Button
      variant={preflight ? "secondary" : "default"}
      disabled={!connected || checking}
      onClick={() => void runPreflight(serverId, preflightSelection(entry, list))}
    >
      {checking ? "Checking the server…" : preflight ? "Check again" : "Check the server"}
    </Button>
  );

  return (
    <div className="flex flex-col gap-5">
      <p className="max-w-3xl text-sm text-slate-600 dark:text-slate-400">
        See what is installed and choose what to add. Nothing is installed or upgraded until you review a plan and approve it.
      </p>

      {!preflight ? (
        <Section title="Check the server first">
          <p className="text-sm text-slate-600 dark:text-slate-400">
            Ossiframe needs to read the server to show what is installed. This only reads and changes nothing.
          </p>
          <div className="flex flex-wrap items-center gap-3">
            {check}
            {!connected && <span className="text-sm text-slate-500">Connect to the server first.</span>}
          </div>
          {showError && <Notice tone="error">{errorMessage(err)}</Notice>}
        </Section>
      ) : (
        <>
          <Section title="Components" aside={`Checked ${localTime(preflight.report.observedAt)}`}>
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
                  <div className="shrink-0 text-right">
                    <StateLabel c={c} state={installState(c, preflight)} />
                  </div>
                </li>
              ))}
            </ul>
            <p className="text-xs text-slate-500">
              Tick only what you want to add. Installed components are left alone. Versions come from the server&apos;s package
              lists as of its last update. Upgrades are not offered yet.
            </p>
          </Section>

          {blocked && (
            <Notice tone="error">
              <p className="font-medium">This server cannot be changed yet. Fix these first, then check again:</p>
              <ul className="mt-1 list-disc space-y-1 pl-5">
                {blockers.map((b) => (
                  <li key={b.id}>{b.message}</li>
                ))}
              </ul>
            </Notice>
          )}
          {showError && <Notice tone="error">{errorMessage(err)}</Notice>}

          <div className="flex flex-wrap items-center gap-3">
            <Button disabled={!connected || planning || blocked || selection.length === 0} onClick={() => void createPlan(serverId, selection)}>
              {planning ? "Creating plan…" : "Review install plan"}
            </Button>
            {check}
            {!connected && <span className="text-sm text-slate-500">Connect to the server to continue.</span>}
            {connected && !blocked && selection.length === 0 && (
              <span className="text-sm text-slate-500">Tick a component to add it.</span>
            )}
            {planCurrent && <span className="text-sm text-slate-600 dark:text-slate-400">Plan ready. Review it on the Plan page.</span>}
          </div>
        </>
      )}
    </div>
  );
}

function StateLabel({ c, state }: { c: ComponentInfo; state: InstallState }) {
  switch (state.kind) {
    case "installed":
      return (
        <div className="flex flex-col items-end gap-0.5">
          <StatusBadge tone="ok">Installed {state.version}</StatusBadge>
          {state.candidate && <StatusBadge tone="neutral">Update available → {state.candidate}</StatusBadge>}
        </div>
      );
    case "notInstalled":
      return <StatusBadge tone="neutral">Not installed</StatusBadge>;
    case "partial":
      return (
        <StatusBadge tone="warn">
          Partly installed ({state.installed} of {state.total} packages)
        </StatusBadge>
      );
    case "problem":
      return <StatusBadge tone="error">{c.title} package needs repair (dpkg state {state.status})</StatusBadge>;
    default:
      return <span className="text-xs text-slate-500">Not checked</span>;
  }
}

function sameSet(a: string[], b: string[]): boolean {
  return a.length === b.length && a.every((x) => b.includes(x));
}
