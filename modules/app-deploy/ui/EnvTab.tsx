import { useEffect, useState } from "react";
import { appsApi, toIpcError, type AppDto, type EnvChange, type EnvEntryDto } from "@ossiframe/client";
import { Button, Input, Mono, Notice, StatusBadge, errorMessage } from "@ossiframe/ui";
import type { Review } from "./AppDetail";
import { Section } from "./parts";

const NEWLINE = /[\r\n]/;

interface Added {
  key: string;
  value: string;
}

/**
 * Environment variables. A secret value never reaches the screen: it shows as hidden and can
 * only be replaced. Nothing is written until the changes are approved as a plan.
 */
export function EnvTab({ app, connected, onReview }: { app: AppDto; connected: boolean; onReview: Review }) {
  const [entries, setEntries] = useState<EnvEntryDto[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  // New values typed by the user for existing keys. A key is here only if it was edited.
  const [edits, setEdits] = useState<Record<string, string>>({});
  // Secret keys whose "Replace value" input is open.
  const [replacing, setReplacing] = useState<Set<string>>(new Set());
  const [deletes, setDeletes] = useState<Set<string>>(new Set());
  const [added, setAdded] = useState<Added[]>([]);
  const [busy, setBusy] = useState(false);

  const [tick, setTick] = useState(0);

  useEffect(() => {
    if (!connected) return;
    let gone = false;
    appsApi.getEnv(app.id).then(
      (list) => {
        if (gone) return;
        setEntries(list);
        setError(null);
      },
      (e: unknown) => !gone && setError(errorMessage(toIpcError(e))),
    );
    return () => {
      gone = true;
    };
  }, [app.id, connected, tick]);

  if (!connected) {
    return (
      <Section title="Environment">
        <Notice tone="warn">Connect to the server to read or change the environment. It is read from the server each time.</Notice>
      </Section>
    );
  }

  const toggle = (set: Set<string>, key: string, on: boolean) => {
    const next = new Set(set);
    if (on) next.add(key);
    else next.delete(key);
    return next;
  };

  // Only changed keys are sent. A secret whose replacement is empty counts as unchanged.
  const changes: EnvChange[] = [];
  for (const e of entries ?? []) {
    if (deletes.has(e.key)) changes.push({ key: e.key, delete: true });
    else if (edits[e.key] !== undefined && (e.secret ? edits[e.key] !== "" : edits[e.key] !== (e.value ?? ""))) {
      changes.push({ key: e.key, value: edits[e.key] ?? "" });
    }
  }
  for (const a of added) if (a.key.trim() !== "") changes.push({ key: a.key.trim(), value: a.value });

  const keys = [...(entries ?? []).map((e) => e.key), ...added.map((a) => a.key.trim())].filter((k) => k !== "");
  const duplicate = keys.find((k, i) => keys.indexOf(k) !== i);
  const hasNewline = changes.some((c) => NEWLINE.test(c.key) || NEWLINE.test(c.value ?? ""));
  const localProblem = duplicate
    ? `The key ${duplicate} appears more than once.`
    : hasNewline
      ? "Keys and values must be a single line. Remove the line break."
      : added.some((a) => a.value !== "" && a.key.trim() === "")
        ? "Give every new variable a key."
        : null;

  async function save() {
    setBusy(true);
    setError(null);
    try {
      const dto = await appsApi.setEnv(app.id, changes);
      const label = (c: EnvChange) =>
        c.delete === true ? `${c.key} (removed)` : (entries ?? []).some((e) => e.key === c.key) ? `${c.key} (changed)` : `${c.key} (added)`;
      onReview(dto, {
        action: "Change environment",
        slug: app.slug,
        appId: app.id,
        rows: [
          ["Changed keys", changes.map(label).join(", ")],
          ["Values", "Not shown. Secret values are never displayed or stored by Ossiframe."],
          ...(app.appType === "laravel" ? ([["Laravel", "The config cache will be refreshed."]] as [string, string][]) : []),
        ],
        creates: [],
        database: false,
      });
    } catch (e) {
      setError(errorMessage(toIpcError(e)));
    }
    setBusy(false);
  }

  function reset() {
    setEdits({});
    setReplacing(new Set());
    setDeletes(new Set());
    setAdded([]);
    setError(null);
  }

  return (
    <Section
      title="Environment"
      aside={
        <Button size="sm" variant="ghost" onClick={() => setTick(tick + 1)}>
          Reload
        </Button>
      }
    >
      {error && <Notice tone="error">{error}</Notice>}
      {localProblem && <Notice tone="warn">{localProblem}</Notice>}
      {entries === null ? (
        !error && <p className="text-sm text-slate-500">Loading…</p>
      ) : entries.length === 0 && added.length === 0 ? (
        <p className="text-sm text-slate-600 dark:text-slate-400">No environment variables found. Add one below.</p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-left text-sm">
            <thead>
              <tr className="text-xs text-slate-500">
                <th className="py-1 pr-3 font-medium">Key</th>
                <th className="py-1 pr-3 font-medium">Value</th>
                <th className="py-1 font-medium" />
              </tr>
            </thead>
            <tbody>
              {entries.map((e) => {
                const removed = deletes.has(e.key);
                return (
                  <tr key={e.key} className="border-t border-slate-200 align-top dark:border-slate-800">
                    <td className={`py-2 pr-3 ${removed ? "line-through opacity-60" : ""}`}>
                      <Mono>{e.key}</Mono>
                    </td>
                    <td className="py-2 pr-3">
                      {removed ? (
                        <span className="text-xs text-slate-500">Will be removed</span>
                      ) : e.secret ? (
                        replacing.has(e.key) ? (
                          <Input
                            type="password"
                            autoComplete="off"
                            aria-label={`New value for ${e.key}`}
                            placeholder="New value"
                            value={edits[e.key] ?? ""}
                            onChange={(ev) => setEdits({ ...edits, [e.key]: ev.target.value })}
                          />
                        ) : (
                          <span className="flex flex-wrap items-center gap-2">
                            <span className="text-slate-500">•••••• (hidden)</span>
                            <StatusBadge tone={e.isSet ? "ok" : "neutral"}>{e.isSet ? "Set" : "Not set"}</StatusBadge>
                          </span>
                        )
                      ) : (
                        <Input
                          aria-label={`Value of ${e.key}`}
                          value={edits[e.key] ?? e.value ?? ""}
                          onChange={(ev) => setEdits({ ...edits, [e.key]: ev.target.value })}
                        />
                      )}
                    </td>
                    <td className="flex flex-wrap justify-end gap-1 py-2">
                      {e.secret && !removed && (
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={() => {
                            const on = !replacing.has(e.key);
                            setReplacing(toggle(replacing, e.key, on));
                            if (!on) {
                              setEdits(Object.fromEntries(Object.entries(edits).filter(([k]) => k !== e.key)));
                            }
                          }}
                        >
                          {replacing.has(e.key) ? "Keep current" : "Replace value"}
                        </Button>
                      )}
                      <Button size="sm" variant="ghost" onClick={() => setDeletes(toggle(deletes, e.key, !removed))}>
                        {removed ? "Undo" : "Delete"}
                      </Button>
                    </td>
                  </tr>
                );
              })}
              {added.map((a, i) => (
                <tr key={`new-${i}`} className="border-t border-slate-200 align-top dark:border-slate-800">
                  <td className="py-2 pr-3">
                    <Input
                      aria-label="New key"
                      placeholder="KEY_NAME"
                      value={a.key}
                      onChange={(ev) => setAdded(added.map((x, n) => (n === i ? { ...x, key: ev.target.value } : x)))}
                    />
                  </td>
                  <td className="py-2 pr-3">
                    <Input
                      aria-label="New value"
                      placeholder="Value"
                      value={a.value}
                      onChange={(ev) => setAdded(added.map((x, n) => (n === i ? { ...x, value: ev.target.value } : x)))}
                    />
                  </td>
                  <td className="py-2 text-right">
                    <Button size="sm" variant="ghost" onClick={() => setAdded(added.filter((_, n) => n !== i))}>
                      Remove row
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <div className="flex flex-wrap gap-2">
        <Button size="sm" variant="secondary" onClick={() => setAdded([...added, { key: "", value: "" }])}>
          Add key
        </Button>
        <Button disabled={busy || changes.length === 0 || localProblem !== null} onClick={() => void save()}>
          {busy ? "Preparing plan…" : "Save changes"}
        </Button>
        <Button variant="ghost" disabled={busy} onClick={reset}>
          Discard
        </Button>
      </div>
      <p className="text-xs text-slate-500">
        Secret values are never shown. You can only replace them. Changes are reviewed and approved before anything is written.
        {app.appType === "laravel" ? " For Laravel, the config cache will be refreshed after the change." : ""}
      </p>
    </Section>
  );
}
