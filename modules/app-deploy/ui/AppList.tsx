import { useEffect, useState } from "react";
import { appsApi, toIpcError, type AppSummaryDto } from "@ossiframe/client";
import { Button, Notice, errorMessage } from "@ossiframe/ui";
import { AppStateBadge, TYPE_LABEL, releaseTime } from "./parts";
import { setView } from "./store";

export function AppList({ serverId, connected }: { serverId: string; connected: boolean }) {
  const [apps, setApps] = useState<AppSummaryDto[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    appsApi.list(serverId).then(
      (l) => {
        setApps(l);
        setError(null);
      },
      (e: unknown) => setError(errorMessage(toIpcError(e))),
    );
  }, [serverId]);

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-base font-semibold">Applications</h2>
        <div className="flex items-center gap-3">
          {!connected && <span className="text-xs text-slate-500">Connect to create an application.</span>}
          <Button disabled={!connected} onClick={() => setView(serverId, { kind: "create" })}>
            New application
          </Button>
        </div>
      </div>
      {error && <Notice tone="error">{error}</Notice>}
      {apps && apps.length === 0 && (
        <div className="flex flex-col items-start gap-2 rounded-lg border border-dashed border-slate-300 p-6 dark:border-slate-700">
          <p className="text-sm font-medium">No applications on this server yet.</p>
          <p className="text-sm text-slate-600 dark:text-slate-400">
            Create one to set up a Linux user, a web site and, for Laravel, a database, then deploy your code from Git. Install the
            software it needs on the Software tab first.
          </p>
        </div>
      )}
      {apps && apps.length > 0 && (
        <div className="overflow-x-auto rounded-md border border-slate-200 dark:border-slate-800">
          <table className="w-full text-left text-sm">
            <thead className="bg-slate-50 text-xs text-slate-500 uppercase dark:bg-slate-900 dark:text-slate-400">
              <tr>
                {["Application", "Type", "Domain", "State", "Current release", "Last deploy"].map((h) => (
                  <th key={h} className="px-3 py-1.5 font-medium">
                    {h}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody className="divide-y divide-slate-200 dark:divide-slate-800">
              {apps.map((a) => (
                <tr key={a.id} className="hover:bg-slate-50 dark:hover:bg-slate-900">
                  <td className="px-3 py-2">
                    <button
                      type="button"
                      className="font-medium text-sky-700 hover:underline dark:text-sky-400"
                      onClick={() => setView(serverId, { kind: "detail", appId: a.id })}
                    >
                      {a.slug}
                    </button>
                  </td>
                  <td className="px-3 py-2">{TYPE_LABEL[a.appType] ?? a.appType}</td>
                  <td className="px-3 py-2 font-mono text-[13px] break-all">{a.domain}</td>
                  <td className="px-3 py-2">
                    <AppStateBadge state={a.state} />
                  </td>
                  <td className="px-3 py-2 font-mono text-[13px]">{a.currentReleaseId ?? "none"}</td>
                  <td className="px-3 py-2">{releaseTime(a.currentReleaseId)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {!apps && !error && <p className="text-sm text-slate-500">Loading…</p>}
    </div>
  );
}
