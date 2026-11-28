import { useEffect, useState, type FormEvent } from "react";
import {
  api,
  appsApi,
  toIpcError,
  type AppTypeInfo,
  type GitHostFingerprint,
  type IpcError,
  type PlanDto,
} from "@ossiframe/client";
import {
  Button,
  Dialog,
  Field,
  Input,
  Mono,
  Notice,
  errorMessage,
} from "@ossiframe/ui";
import { ApprovePlanDialog, type PlanSummary } from "./ApprovePlanDialog";
import { TYPE_LABEL } from "./parts";
import { setView } from "./store";

const SLUG_MAX = 27;
const SELECT =
  "h-9 w-full rounded-md border border-slate-300 bg-white px-2 text-sm text-slate-900 focus-visible:outline-2 focus-visible:outline-sky-600 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-100";

/** `My Shop!` becomes `my-shop`. Only a suggestion; the user can edit it. */
function suggestSlug(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, SLUG_MAX)
    .replace(/-+$/, "");
}

const SLUG_RE = /^[a-z0-9]([a-z0-9-]*[a-z0-9])?$/;

/**
 * The server host as dashed IPv4 (`203-0-113-10`) when it is a public IPv4
 * address, else null. sslip.io maps `<anything>.<a-b-c-d>.sslip.io` to that
 * address, so it only works for an IP that is reachable from the internet.
 */
function publicIpv4Dashed(host: string): string | null {
  const m = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(host.trim());
  if (!m) return null;
  const o = m.slice(1).map(Number);
  if (o.some((n) => n > 255)) return null;
  const a = o[0] ?? 0;
  const b = o[1] ?? 0;
  const isPrivate =
    a === 10 ||
    a === 127 ||
    a === 0 ||
    (a === 169 && b === 254) ||
    (a === 172 && b >= 16 && b <= 31) ||
    (a === 192 && b === 168) ||
    (a === 100 && b >= 64 && b <= 127) ||
    a >= 224;
  return isPrivate ? null : o.join("-");
}

/** A free public test name for the app, for example `shop.203-0-113-10.sslip.io`. */
export function testAddress(slug: string, host: string): string | null {
  const ip = publicIpv4Dashed(host);
  return ip && slug ? `${slug}.${ip}.sslip.io` : null;
}

/** Backend field names mapped to the form. Anything else shows as a general error. */
const FIELD: Record<string, "slug" | "domain" | "repo" | "branch" | "entry"> = {
  slug: "slug",
  domain: "domain",
  repositoryUrl: "repo",
  branch: "branch",
  entryFile: "entry",
};

type FieldErrors = Partial<
  Record<"slug" | "domain" | "repo" | "branch" | "entry", string>
>;

export function CreateApp({
  serverId,
  connected,
}: {
  serverId: string;
  connected: boolean;
}) {
  const [types, setTypes] = useState<AppTypeInfo[] | null>(null);
  const [typesError, setTypesError] = useState<string | null>(null);
  const [typeId, setTypeId] = useState("laravel");
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [slugEdited, setSlugEdited] = useState(false);
  const [domain, setDomain] = useState("");
  const [repoUrl, setRepoUrl] = useState("");
  const [branch, setBranch] = useState("main");
  const [entry, setEntry] = useState<string | null>(null);
  const [withDb, setWithDb] = useState(false);
  const [disableDefault, setDisableDefault] = useState(false);

  const [busy, setBusy] = useState(false);
  const [errors, setErrors] = useState<FieldErrors>({});
  const [general, setGeneral] = useState<string | null>(null);
  const [fingerprints, setFingerprints] = useState<GitHostFingerprint[] | null>(
    null,
  );
  const [review, setReview] = useState<{
    dto: PlanDto;
    summary: PlanSummary;
  } | null>(null);

  useEffect(() => {
    appsApi
      .types()
      .then(setTypes, (e: unknown) =>
        setTypesError(errorMessage(toIpcError(e))),
      );
  }, []);

  // Only used to offer a test address; a failure just hides the button.
  const [serverHost, setServerHost] = useState<string | null>(null);
  useEffect(() => {
    api.listServers().then(
      (list) =>
        setServerHost(list.find((s) => s.id === serverId)?.host ?? null),
      () => setServerHost(null),
    );
  }, [serverId]);
  const suggestedTest = serverHost ? testAddress(slug, serverHost) : null;

  const info = types?.find((t) => t.id === typeId) ?? null;
  const entryValue = entry ?? info?.entryFile ?? "";
  const usesDb =
    info?.database === "default" || (info?.database === "optIn" && withDb);

  function onName(v: string) {
    setName(v);
    if (!slugEdited) setSlug(suggestSlug(v));
  }

  function pickType(id: string) {
    setTypeId(id);
    setEntry(null);
    setWithDb(false);
  }

  const slugHint =
    slug && !SLUG_RE.test(slug)
      ? "Use only lowercase letters, numbers and hyphens (not at the start or end)."
      : slug.length > SLUG_MAX
        ? `At most ${SLUG_MAX} characters (now ${slug.length}).`
        : undefined;
  const ready =
    Boolean(info) &&
    slug !== "" &&
    !slugHint &&
    domain.trim() !== "" &&
    repoUrl.trim() !== "" &&
    branch.trim() !== "";

  async function submit(trusted: string[] | null) {
    if (!info) return;
    setBusy(true);
    setErrors({});
    setGeneral(null);
    try {
      const dto = await appsApi.createPlan({
        serverId,
        appType: info.id,
        slug,
        domain: domain.trim(),
        repoUrl: repoUrl.trim(),
        repoBranch: branch.trim(),
        entryFile: info.entryFileFixed ? null : entryValue.trim(),
        withDatabase: usesDb,
        disableDefaultSite: disableDefault,
        trustedGitHostFingerprints: trusted,
      });
      setFingerprints(null);
      setReview({
        dto,
        summary: {
          action: "Create application",
          slug,
          appId: dto.plan.target.appId ?? null,
          rows: [
            ["Type", info.title],
            ["Domain", domain.trim()],
            ["Repository", `${repoUrl.trim()} (branch ${branch.trim()})`],
            ["Entry file", entryValue],
            ["Database", usesDb ? "A MySQL database for this app" : "None"],
            [
              "NGINX default site",
              disableDefault ? "Disabled" : "Left as it is",
            ],
          ],
          creates: [
            `A Linux user ossi-${slug} that owns and runs the application`,
            ...(usesDb
              ? ["A MySQL database and database user for this application"]
              : []),
          ],
          database: usesDb,
        },
      });
    } catch (e) {
      handleError(toIpcError(e));
    }
    setBusy(false);
  }

  function handleError(e: IpcError) {
    if (
      e.kind === "plan" &&
      e.reason === "gitHostKeyUnconfirmed" &&
      e.fingerprints?.length
    ) {
      setFingerprints(e.fingerprints);
      return;
    }
    const msg = errorMessage(e);
    if (e.kind === "validation") {
      const f = FIELD[e.field];
      if (f) return setErrors({ [f]: e.message });
    } else if (e.kind === "plan" && e.reason === "slugInUse") {
      return setErrors({ slug: msg });
    } else if (e.kind === "plan" && e.reason === "domainInUse") {
      return setErrors({ domain: msg });
    }
    setGeneral(msg);
  }

  function onSubmit(ev: FormEvent) {
    ev.preventDefault();
    if (ready && !busy) void submit(null);
  }

  if (typesError) return <Notice tone="error">{typesError}</Notice>;
  if (!types) return <p className="text-sm text-slate-500">Loading…</p>;

  return (
    <form onSubmit={onSubmit} className="flex max-w-2xl flex-col gap-5">
      <div className="flex items-center justify-between">
        <h2 className="text-base font-semibold">New application</h2>
        <Button
          variant="ghost"
          onClick={() => setView(serverId, { kind: "list" })}
        >
          Back to applications
        </Button>
      </div>
      {!connected && (
        <Notice tone="warn">
          Connect to the server to create an application.
        </Notice>
      )}
      {general && <Notice tone="error">{general}</Notice>}

      <fieldset className="flex flex-col gap-2">
        <legend className="mb-1 text-xs font-medium text-slate-700 dark:text-slate-300">
          What are you deploying?
        </legend>
        {types.map((t) => (
          <label
            key={t.id}
            className="flex cursor-pointer items-start gap-3 rounded-md border border-slate-200 p-3 has-[:checked]:border-sky-600 dark:border-slate-800"
          >
            <input
              type="radio"
              name="app-type"
              className="mt-1"
              checked={t.id === typeId}
              onChange={() => pickType(t.id)}
            />
            <span className="flex flex-col">
              <span className="text-sm font-medium">
                {TYPE_LABEL[t.id] ?? t.title}
              </span>
              <span className="text-xs text-slate-600 dark:text-slate-400">
                {t.description}
              </span>
            </span>
          </label>
        ))}
      </fieldset>

      <div className="grid gap-4 sm:grid-cols-2">
        <Field label="Name" hint="A friendly name, for example My Shop.">
          {(p) => (
            <Input
              {...p}
              value={name}
              onChange={(e) => onName(e.target.value)}
              autoFocus
            />
          )}
        </Field>
        <Field
          label="Short name (slug)"
          error={errors.slug ?? slugHint}
          hint="Used for the Linux user, folders and site name. Cannot be changed later."
        >
          {(p) => (
            <Input
              {...p}
              className="font-mono"
              value={slug}
              onChange={(e) => {
                setSlug(e.target.value);
                setSlugEdited(true);
              }}
            />
          )}
        </Field>
      </div>

      <Field
        label="Domain"
        error={errors.domain}
        hint={
          suggestedTest
            ? "The host name the site answers to, for example shop.example.com. No domain yet? A test address uses the free sslip.io DNS service, which maps the name to this server's IP. Anyone who knows it can open the site, over HTTP only. Use it for testing, not production."
            : "The host name the site answers to, for example shop.example.com."
        }
      >
        {(p) => (
          <div className="flex gap-2">
            <Input
              {...p}
              value={domain}
              onChange={(e) => setDomain(e.target.value)}
              placeholder="shop.example.com"
            />
            {suggestedTest && (
              <Button
                type="button"
                variant="secondary"
                onClick={() => setDomain(suggestedTest)}
              >
                Use a test address
              </Button>
            )}
          </div>
        )}
      </Field>

      <Field
        label="Repository URL"
        error={errors.repo}
        hint="A public https:// URL works right away. A private repository uses an SSH URL such as git@github.com:owner/repo.git and a deploy key you add afterwards."
      >
        {(p) => (
          <Input
            {...p}
            value={repoUrl}
            onChange={(e) => setRepoUrl(e.target.value)}
            placeholder="https://github.com/owner/repo.git"
          />
        )}
      </Field>

      <div className="grid gap-4 sm:grid-cols-2">
        <Field label="Branch" error={errors.branch}>
          {(p) => (
            <Input
              {...p}
              value={branch}
              onChange={(e) => setBranch(e.target.value)}
            />
          )}
        </Field>
        {info && !info.entryFileFixed && (
          <Field
            label="Entry file"
            error={errors.entry}
            hint="The file the web server starts from."
          >
            {(p) =>
              info.id === "php" ? (
                <select
                  {...p}
                  className={SELECT}
                  value={entryValue}
                  onChange={(e) => setEntry(e.target.value)}
                >
                  <option value="public/index.php">public/index.php</option>
                  <option value="index.php">index.php</option>
                </select>
              ) : (
                <Input
                  {...p}
                  className="font-mono"
                  value={entryValue}
                  onChange={(e) => setEntry(e.target.value)}
                />
              )
            }
          </Field>
        )}
      </div>

      {info?.database === "default" && (
        <p className="text-sm text-slate-600 dark:text-slate-400">
          <span className="font-medium text-slate-900 dark:text-slate-100">
            Database included.
          </span>{" "}
          A MySQL database and user are created for this application. The
          password is generated on the server and kept only in its .env file.
        </p>
      )}
      {info?.database === "optIn" && (
        <label className="flex items-start gap-2 text-sm">
          <input
            type="checkbox"
            className="mt-1"
            checked={withDb}
            onChange={(e) => setWithDb(e.target.checked)}
          />
          <span>
            Use a MySQL database
            <span className="block text-xs text-slate-600 dark:text-slate-400">
              Creates a database and user for this app. The password is kept
              only on the server.
            </span>
          </span>
        </label>
      )}

      <label className="flex items-start gap-2 text-sm">
        <input
          type="checkbox"
          className="mt-1"
          checked={disableDefault}
          onChange={(e) => setDisableDefault(e.target.checked)}
        />
        <span>
          Disable the NGINX default site
          <span className="block text-xs text-slate-600 dark:text-slate-400">
            Off by default. Turn it on if this is the only site on the server
            and requests for unknown domains should not reach the NGINX welcome
            page. The default site file is kept, only its link is removed.
          </span>
        </span>
      </label>

      <div className="flex items-center gap-3">
        <Button type="submit" disabled={!ready || busy || !connected}>
          {busy ? "Preparing plan…" : "Review plan"}
        </Button>
        <span className="text-xs text-slate-500">
          Nothing is changed on the server until you approve the plan.
        </span>
      </div>

      {fingerprints && (
        <GitHostDialog
          fingerprints={fingerprints}
          busy={busy}
          onCancel={() => setFingerprints(null)}
          onTrust={() => void submit(fingerprints.map((f) => f.sha256))}
        />
      )}
      {review && (
        <ApprovePlanDialog
          serverId={serverId}
          dto={review.dto}
          summary={review.summary}
          connected={connected}
          onClose={() => setReview(null)}
        />
      )}
    </form>
  );
}

/** Like the server host key dialog, for the Git host's SSH key. */
function GitHostDialog({
  fingerprints,
  busy,
  onCancel,
  onTrust,
}: {
  fingerprints: GitHostFingerprint[];
  busy: boolean;
  onCancel: () => void;
  onTrust: () => void;
}) {
  return (
    <Dialog
      open
      onOpenChange={(o) => !o && !busy && onCancel()}
      title="Verify the Git host's fingerprint"
      description="This Git host is not one Ossiframe already knows. Its identity has not been verified. Nothing has been created yet."
      footer={
        <>
          <Button variant="secondary" onClick={onCancel} disabled={busy}>
            Cancel
          </Button>
          <Button onClick={onTrust} disabled={busy}>
            {busy ? "Checking…" : "Trust and continue"}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3 text-sm">
        {fingerprints.map((f) => (
          <div
            key={`${f.host}:${f.port}:${f.keyType}`}
            className="flex flex-col gap-1"
          >
            <div className="text-xs text-slate-500">
              Host{" "}
              <Mono>
                {f.host}:{f.port}
              </Mono>
              , key type <Mono>{f.keyType}</Mono>
            </div>
            <Mono className="block rounded bg-slate-100 p-2 dark:bg-slate-800">
              {f.sha256}
            </Mono>
          </div>
        ))}
        <p className="text-slate-600 dark:text-slate-400">
          Verify this with your Git provider. Compare the fingerprint with the
          one the provider publishes for its SSH keys. Trusting it lets this
          application's server connect only to a host presenting this key.
        </p>
      </div>
    </Dialog>
  );
}
