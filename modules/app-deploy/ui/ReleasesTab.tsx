import { useState } from "react";
import { appsApi, toIpcError, type AppDto, type ReleaseDto } from "@ossiframe/client";
import { Button, Mono, Notice, StatusBadge, errorMessage, type Tone } from "@ossiframe/ui";
import type { Review } from "./AppDetail";
import type { PlanSummary } from "./ApprovePlanDialog";
import { Section, releaseTime } from "./parts";

const STATUS: Record<string, [Tone, string]> = {
  pending: ["busy", "In progress"],
  deployed: ["ok", "Deployed"],
  failed: ["error", "Failed"],
  rolledBackFrom: ["neutral", "Rolled back from"],
  rolledBackTo: ["ok", "Rolled back to"],
};

/** A release that went live before and is not the current one can be switched back to. */
function eligible(r: ReleaseDto, currentId: string | null): boolean {
  return r.id !== currentId && (r.status === "deployed" || r.status === "rolledBackTo");
}

const newestFirst = (a: ReleaseDto, b: ReleaseDto) => b.id.localeCompare(a.id);

/** The newest release that can be rolled back to, if any. */
export function previousRelease(releases: ReleaseDto[], currentId: string | null): ReleaseDto | null {
  return [...releases].sort(newestFirst).find((r) => eligible(r, currentId)) ?? null;
}

export function rollbackSummary(app: AppDto, release: ReleaseDto): PlanSummary {
  return {
    action: "Roll back",
    slug: app.slug,
    appId: app.id,
    rows: [
      ["Domain", app.domain],
      ["Roll back to", `${release.id}${release.commitSha ? ` (commit ${release.commitSha.slice(0, 7)})` : ""}`],
      ["Database", "Not changed. Rolling back does not undo database migrations."],
    ],
    creates: [],
    database: false,
  };
}

export function ReleasesTab({
  app,
  releases,
  connected,
  onReview,
}: {
  app: AppDto;
  releases: ReleaseDto[];
  connected: boolean;
  onReview: Review;
}) {
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const currentId = app.currentRelease?.id ?? null;

  async function rollback(r: ReleaseDto) {
    setBusy(r.id);
    setError(null);
    try {
      onReview(await appsApi.rollbackPlan(app.id, r.id), rollbackSummary(app, r));
    } catch (e) {
      setError(errorMessage(toIpcError(e)));
    }
    setBusy(null);
  }

  return (
    <Section title="Releases">
      {error && <Notice tone="error">{error}</Notice>}
      {releases.length === 0 ? (
        <p className="text-sm text-slate-600 dark:text-slate-400">No releases yet. Each deploy creates one, and the last few are kept for rollback.</p>
      ) : (
        <ul className="flex flex-col divide-y divide-slate-200 dark:divide-slate-800">
          {[...releases].sort(newestFirst).map((r) => {
            const [tone, label] = STATUS[r.status] ?? ["neutral" as Tone, r.status];
            return (
              <li key={r.id} className="flex flex-wrap items-center justify-between gap-3 py-2 text-sm">
                <div className="flex flex-col gap-0.5">
                  <span className="flex flex-wrap items-center gap-2">
                    <Mono>{r.id}</Mono>
                    {r.id === currentId && <StatusBadge tone="ok">Current</StatusBadge>}
                    <StatusBadge tone={tone}>{label}</StatusBadge>
                  </span>
                  <span className="text-xs text-slate-500">
                    {releaseTime(r.id)}. Branch {r.branch}. Commit {r.commitSha ? r.commitSha.slice(0, 7) : "unknown"}.
                  </span>
                </div>
                {eligible(r, currentId) && (
                  <Button size="sm" variant="secondary" disabled={!connected || busy !== null} onClick={() => void rollback(r)}>
                    {busy === r.id ? "Preparing plan…" : "Roll back to this release"}
                  </Button>
                )}
              </li>
            );
          })}
        </ul>
      )}
      <p className="text-xs text-slate-500">Rolling back switches the site to an earlier release. It does not undo database changes.</p>
    </Section>
  );
}
