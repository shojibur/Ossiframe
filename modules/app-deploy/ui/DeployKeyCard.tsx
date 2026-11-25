import { useEffect, useState } from "react";
import { appsApi, toIpcError, type AccessCheckDto, type AccessResult, type DeployKeyStatus } from "@ossiframe/client";
import { Button, Mono, Notice, errorMessage } from "@ossiframe/ui";
import { Section } from "./parts";

const RESULT: Record<AccessResult, string> = {
  ok: "Access works. The repository is readable and the branch exists.",
  authDenied: "The key was not accepted yet. Add the public key above to the repository as a deploy key, then check again.",
  branchMissing: "The repository was reached, but the branch does not exist. Check the branch name.",
  repoNotFound: "The repository was not found, or the key cannot see it. Check the repository URL and that the key was added to this repository.",
  hostKeyFailed: "The Git host's key could not be verified, so the connection was refused. Nothing was sent to it.",
  privateHttps: "This looks like a private repository over HTTPS. Use an SSH URL (git@host:owner/repo.git) so a deploy key can be used.",
  unreachable: "The server could not reach the Git host. Check the server's network and try again.",
  failed: "The check failed for another reason. Try again, or check the server.",
};

/**
 * Step-by-step setup for a private repository: copy the key, add it at the Git host,
 * check access, then deploy. `onDeploy` stays disabled until the check passes.
 */
export function DeployKeyCard({
  appId,
  connected,
  onDeploy,
  deploying,
}: {
  appId: string;
  connected: boolean;
  onDeploy: () => void;
  deploying: boolean;
}) {
  const [status, setStatus] = useState<DeployKeyStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [checking, setChecking] = useState(false);
  const [result, setResult] = useState<AccessCheckDto | null>(null);

  useEffect(() => {
    if (!connected) return;
    appsApi.deployKeyStatus(appId).then(
      (s) => {
        setStatus(s);
        setError(null);
      },
      (e: unknown) => setError(errorMessage(toIpcError(e))),
    );
  }, [appId, connected]);

  async function copy() {
    if (!status) return;
    try {
      await navigator.clipboard.writeText(status.publicKey);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      setError("Copy failed. Select the key and copy it by hand.");
    }
  }

  async function check() {
    setChecking(true);
    setError(null);
    try {
      setResult(await appsApi.checkAccess(appId));
    } catch (e) {
      setResult(null);
      setError(errorMessage(toIpcError(e)));
    }
    setChecking(false);
  }

  const ok = result?.passed === true;
  return (
    <Section title="Set up access to your private repository">
      {!connected && <Notice tone="warn">Connect to the server to set up the deploy key.</Notice>}
      {error && <Notice tone="error">{error}</Notice>}
      <ol className="flex flex-col gap-5 text-sm">
        <li className="flex flex-col gap-2">
          <span className="font-medium">1. Copy this public key</span>
          {status ? (
            <>
              <Mono className="block rounded-md border border-slate-200 bg-slate-50 p-2 dark:border-slate-800 dark:bg-slate-900">
                {status.publicKey}
              </Mono>
              <div>
                <Button size="sm" variant="secondary" onClick={() => void copy()}>
                  {copied ? "Copied" : "Copy key"}
                </Button>
              </div>
            </>
          ) : (
            <span className="text-slate-500">{connected ? "Loading the key…" : "Not available while disconnected."}</span>
          )}
          <span className="text-xs text-slate-500">This is the public half. The private key stays on the server.</span>
        </li>
        <li className="flex flex-col gap-1">
          <span className="font-medium">2. Add it to the repository{status ? ` on ${status.host}` : ""}</span>
          <span className="text-slate-600 dark:text-slate-400">{status?.instructions ?? "Instructions appear once the key is loaded."}</span>
        </li>
        <li className="flex flex-col gap-2">
          <span className="font-medium">3. Check access</span>
          <div>
            <Button variant="secondary" disabled={!connected || checking || !status} onClick={() => void check()}>
              {checking ? "Checking…" : "Check access"}
            </Button>
          </div>
          {result && <Notice tone={ok ? "info" : "warn"}>{RESULT[result.result]}</Notice>}
        </li>
        <li className="flex flex-col gap-2">
          <span className="font-medium">4. Deploy</span>
          <div>
            <Button disabled={!ok || !connected || deploying} onClick={onDeploy}>
              {deploying ? "Preparing plan…" : "Deploy"}
            </Button>
          </div>
          {!ok && <span className="text-xs text-slate-500">Available after the access check passes.</span>}
        </li>
      </ol>
    </Section>
  );
}
