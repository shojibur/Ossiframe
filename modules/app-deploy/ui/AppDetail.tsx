import { useEffect, useState, type ReactNode } from "react";
import {
  appsApi,
  provisioningApi,
  toIpcError,
  type AppDto,
  type OperationSummaryDto,
  type PlanDto,
  type ReleaseDto,
} from "@ossiframe/client";
import { Button, Field, Input, KeyValue, Mono, Notice, errorMessage, localTime } from "@ossiframe/ui";
import { ApprovePlanDialog, type PlanSummary } from "./ApprovePlanDialog";
import { DeployKeyCard } from "./DeployKeyCard";
import { EnvTab } from "./EnvTab";
import { ReleasesTab, previousRelease, rollbackSummary } from "./ReleasesTab";
import { RemoveTab } from "./RemoveTab";
import { AppStateBadge, OperationBadge, Section, TYPE_LABEL, releaseTime } from "./parts";
import { setView } from "./store";

type Tab = "overview" | "deploy" | "releases" | "environment" | "remove";
const TABS: [Tab, string][] = [
  ["overview", "Overview"],
  ["deploy", "Deploy"],
  ["releases", "Releases"],
  ["environment", "Environment"],
  ["remove", "Remove"],
];

/** Passed to each tab so every change goes through the same review and approval dialog. */
export type Review = (dto: PlanDto, summary: PlanSummary) => void;

/** Newest application operation for this app. Looks at the few most recent ones only. */
async function findLastOperation(serverId: string, appId: string): Promise<OperationSummaryDto | null> {
  const list = (await provisioningApi.listOperations(serverId, 50))
    .filter((o) => o.kind === "app-deploy")
    .sort((a, b) => b.createdAt.localeCompare(a.createdAt))
    .slice(0, 8);
  for (const o of list) {
    const detail = await provisioningApi.getOperation(o.id);
    if (detail.plan?.target.appId === appId) return o;
  }
  return null;
}

export function AppDetail({ serverId, appId, connected }: { serverId: string; appId: string; connected: boolean }) {
  const [app, setApp] = useState<AppDto | null>(null);
  const [releases, setReleases] = useState<ReleaseDto[]>([]);
  const [lastOp, setLastOp] = useState<OperationSummaryDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>("overview");
  const [review, setReview] = useState<{ dto: PlanDto; summary: PlanSummary } | null>(null);

  // The screen is remounted when an operation ends and the user returns, so this
  // reloads the app and its releases (the environment tab loads its own data).
  useEffect(() => {
    let gone = false;
    Promise.all([appsApi.get(appId), appsApi.listReleases(appId)]).then(
      ([a, r]) => {
        if (gone) return;
        setApp(a);
        setReleases(r);
        setError(null);
      },
      (e: unknown) => !gone && setError(errorMessage(toIpcError(e))),
    );
    findLastOperation(serverId, appId).then(
      (o) => !gone && setLastOp(o),
      () => !gone && setLastOp(null),
    );
    return () => {
      gone = true;
    };
  }, [appId, serverId]);

  const back = () => setView(serverId, { kind: "list" });
  if (!app) {
    return (
      <div className="flex flex-col gap-3">
        {error && <Notice tone="error">{error}</Notice>}
        <div>
          <Button variant="ghost" onClick={back}>
            Back to applications
          </Button>
        </div>
        {!error && <p className="text-sm text-slate-500">Loading…</p>}
      </div>
    );
  }

  const onReview: Review = (dto, summary) => setReview({ dto, summary });
  return (
    <div className="flex max-w-3xl flex-col gap-5">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex flex-col gap-1">
          <h2 className="text-base font-semibold">{app.slug}</h2>
          <AppStateBadge state={app.state} />
        </div>
        <Button variant="ghost" onClick={back}>
          Back to applications
        </Button>
      </div>
      {error && <Notice tone="error">{error}</Notice>}
      <div role="tablist" aria-label="Application" className="flex flex-wrap gap-1 border-b border-slate-200 dark:border-slate-800">
        {TABS.map(([id, label]) => (
          <button
            key={id}
            role="tab"
            type="button"
            aria-selected={tab === id}
            onClick={() => setTab(id)}
            className="-mb-px border-b-2 border-transparent px-3 py-2 text-sm text-slate-600 hover:text-slate-900 aria-selected:border-sky-600 aria-selected:font-medium aria-selected:text-slate-900 dark:text-slate-400 dark:hover:text-slate-100 dark:aria-selected:text-slate-100"
          >
            {label}
          </button>
        ))}
      </div>
      {tab === "overview" && <Overview app={app} releases={releases} lastOp={lastOp} onReview={onReview} setTab={setTab} connected={connected} />}
      {tab === "deploy" && <DeployTab app={app} connected={connected} onReview={onReview} />}
      {tab === "releases" && <ReleasesTab app={app} releases={releases} connected={connected} onReview={onReview} />}
      {tab === "environment" && <EnvTab app={app} connected={connected} onReview={onReview} />}
      {tab === "remove" && <RemoveTab app={app} connected={connected} onReview={onReview} />}
      {review && (
        <ApprovePlanDialog
          serverId={serverId}
          dto={review.dto}
          summary={review.summary}
          connected={connected}
          onClose={() => setReview(null)}
        />
      )}
    </div>
  );
}

function Overview({
  app,
  releases,
  lastOp,
  onReview,
  setTab,
  connected,
}: {
  app: AppDto;
  releases: ReleaseDto[];
  lastOp: OperationSummaryDto | null;
  onReview: Review;
  setTab: (t: Tab) => void;
  connected: boolean;
}) {
  const cur = app.currentRelease;
  const prev = previousRelease(releases, cur?.id ?? null);
  // The switch happened, then a check failed: the new release is live and the app is marked failed.
  const verifyFailed = app.state === "failed" && cur !== null && lastOp?.state === "failed" && cur.operationId === lastOp.id;
  return (
    <div className="flex flex-col gap-5">
      {lastOp?.state === "unknownOutcome" && (
        <Notice tone="warn">
          The last operation ({localTime(lastOp.createdAt)}) has an unknown outcome. Ossiframe lost track of it and did not retry
          it. Open the Operation tab at the top of this server's page to Reconcile it or choose "Mark resolved". Deploys and
          changes stay blocked until then.
        </Notice>
      )}
      {verifyFailed && prev && (
        <RollbackPrompt app={app} previous={prev} connected={connected} onReview={onReview} />
      )}
      {app.state === "failed" && !verifyFailed && lastOp?.state !== "unknownOutcome" && (
        <Notice tone="warn">
          This application needs attention. Check the last operation in the Operation tab or History tab to see what failed.
        </Notice>
      )}
      <Section title="Application">
        <KeyValue
          rows={[
            ["Address", <Mono key="d">http://{app.domain}</Mono>],
            ["Type", TYPE_LABEL[app.appType] ?? app.appType],
            ["State", <AppStateBadge key="s" state={app.state} />],
            ["Repository", <Mono key="r">{app.repoUrl}</Mono>],
            ["Database", app.hasDatabase ? "Yes, included" : "No"],
            [
              "Last operation",
              lastOp ? (
                <span key="o" className="flex flex-wrap items-center gap-2">
                  <OperationBadge state={lastOp.state} />
                  <span className="text-xs text-slate-500">{localTime(lastOp.createdAt)}</span>
                </span>
              ) : (
                "None yet"
              ),
            ],
          ]}
        />
        {app.hasDatabase && (
          <p className="text-xs text-slate-500">
            The database password is kept only in the application's .env file on the server. Ossiframe never shows it.
          </p>
        )}
      </Section>
      <Section title="Current release">
        {cur ? (
          <KeyValue
            rows={[
              ["Release", <Mono key="i">{cur.id}</Mono>],
              ["Commit", cur.commitSha ? <Mono key="c">{cur.commitSha.slice(0, 7)}</Mono> : "unknown"],
              ["Branch", cur.branch],
              ["Deployed", releaseTime(cur.id)],
            ]}
          />
        ) : (
          <p className="text-sm text-slate-600 dark:text-slate-400">Nothing is deployed yet. Open the Deploy tab to put the site online.</p>
        )}
        <div>
          <Button variant="secondary" size="sm" onClick={() => setTab("deploy")}>
            Go to Deploy
          </Button>
        </div>
      </Section>
    </div>
  );
}

/** Primary action after a deploy whose checks failed after the switch. */
function RollbackPrompt({
  app,
  previous,
  connected,
  onReview,
}: {
  app: AppDto;
  previous: ReleaseDto;
  connected: boolean;
  onReview: Review;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function go() {
    setBusy(true);
    setError(null);
    try {
      onReview(await appsApi.rollbackPlan(app.id, previous.id), rollbackSummary(app, previous));
    } catch (e) {
      setError(errorMessage(toIpcError(e)));
    }
    setBusy(false);
  }
  return (
    <Notice tone="warn">
      <div className="flex flex-col gap-2">
        <span className="font-medium">Verification failed. Roll back to the previous release?</span>
        <span>
          The new release is live but a check failed afterwards. Ossiframe does not roll back by itself. The previous release is{" "}
          <Mono>{previous.id}</Mono>.
        </span>
        {error && <span className="text-red-800 dark:text-red-300">{error}</span>}
        <div>
          <Button disabled={!connected || busy} onClick={() => void go()}>
            {busy ? "Preparing plan…" : "Roll back to previous release"}
          </Button>
        </div>
      </div>
    </Notice>
  );
}

function DeployTab({ app, connected, onReview }: { app: AppDto; connected: boolean; onReview: Review }) {
  const [branch, setBranch] = useState("");
  const [migrate, setMigrate] = useState(false);
  const [planning, setPlanning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const canMigrate = app.appType === "laravel" && app.hasDatabase;
  const never = app.currentRelease === null;

  async function deploy() {
    setPlanning(true);
    setError(null);
    const override = branch.trim();
    const useBranch = override === "" ? null : override;
    try {
      const dto = await appsApi.redeployPlan(app.id, useBranch, canMigrate && migrate);
      onReview(dto, {
        action: "Deploy",
        slug: app.slug,
        appId: app.id,
        rows: [
          ["Domain", app.domain],
          ["Repository", `${app.repoUrl} (branch ${useBranch ?? app.repoBranch})`],
          ["Database migrations", canMigrate ? (migrate ? "Run" : "Not run") : "Not applicable"],
        ],
        creates: [],
        database: false,
      });
    } catch (e) {
      setError(errorMessage(toIpcError(e)));
    }
    setPlanning(false);
  }

  const options: ReactNode = (
    <div className="flex flex-col gap-3">
      <Field label="Branch (optional)" hint={`Leave empty to use ${app.repoBranch}. This applies to this deploy only.`}>
        {(p) => <Input {...p} value={branch} placeholder={app.repoBranch} onChange={(e) => setBranch(e.target.value)} />}
      </Field>
      {canMigrate && (
        <label className="flex items-start gap-2 text-sm">
          <input type="checkbox" className="mt-1" checked={migrate} onChange={(e) => setMigrate(e.target.checked)} />
          <span>
            Run database migrations
            <span className="block text-xs text-amber-800 dark:text-amber-300">
              Migrations change your database and cannot be undone by a rollback. Leave this off unless you need them.
            </span>
          </span>
        </label>
      )}
    </div>
  );

  return (
    <div className="flex flex-col gap-5">
      {error && <Notice tone="error">{error}</Notice>}
      {app.needsDeployKey && never ? (
        <>
          {options}
          <DeployKeyCard appId={app.id} connected={connected} onDeploy={() => void deploy()} deploying={planning} />
        </>
      ) : (
        <Section title="Deploy">
          <p className="text-sm">
            {never
              ? "The application is set up. Deploy to fetch the code from the repository and put the site online."
              : "Fetch the latest code from the repository and switch the site to the new release. The previous release is kept for rollback."}
          </p>
          {options}
          <div>
            <Button disabled={!connected || planning} onClick={() => void deploy()}>
              {planning ? "Preparing plan…" : "Deploy now"}
            </Button>
            {!connected && <span className="ml-3 text-sm text-slate-500">Connect to the server first.</span>}
          </div>
        </Section>
      )}
    </div>
  );
}
