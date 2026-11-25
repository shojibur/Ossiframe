import { Button, KeyValue, Notice, errorMessage, localTime } from "@ossiframe/ui";
import { CLASS_LABEL, Section, SUDO_LABEL, mb, useComponentList } from "./parts";
import { preflightSelection, runPreflight, useServerEntry } from "./store";

export function PreflightPage({ serverId, connected }: { serverId: string; connected: boolean }) {
  const entry = useServerEntry(serverId);
  const { list, error: listError } = useComponentList();
  const running = entry.busy === "preflight";
  const dto = entry.preflight;
  const report = dto?.report;
  const blockers = report?.conflicts.filter((c) => c.severity === "blocker") ?? [];
  const warnings = report?.conflicts.filter((c) => c.severity === "warning") ?? [];

  return (
    <div className="flex flex-col gap-5">
      <p className="max-w-3xl text-sm text-slate-600 dark:text-slate-400">
        Preflight reads the server and tells you whether software can be installed. It changes nothing. The only command run with
        elevated rights is a check that passwordless sudo works. Installing needs sudo without a password, which gives this user
        root access.
      </p>

      <div className="flex gap-2">
        <Button disabled={!connected || running || !list} onClick={() => list && void runPreflight(serverId, preflightSelection(entry, list))}>
          {running ? "Running preflight…" : dto ? "Run again" : "Run preflight"}
        </Button>
      </div>
      {!connected && <p className="text-sm text-slate-500">Connect to the server to run preflight.</p>}
      {connected && !report && !running && !entry.error && (
        <p className="text-sm text-slate-500">Preflight has not run yet for this connection.</p>
      )}
      {listError && <Notice tone="error">{listError}</Notice>}
      {entry.error && entry.busy === null && !entry.plan && <Notice tone="error">{errorMessage(entry.error)}</Notice>}

      {report && dto && (
        <>
          <Section title="Result" aside={`Observed ${localTime(report.observedAt)}`}>
            {dto.canProvision ? (
              <Notice tone="info">Nothing is blocking. Choose what to install on the Software page.</Notice>
            ) : (
              <Notice tone="error">Installing is blocked until the problems below are fixed.</Notice>
            )}
            <KeyValue
              rows={[
                ["Server class", CLASS_LABEL[report.serverClass]],
                ["Operating system", report.os],
                ["sudo", SUDO_LABEL[report.sudo]],
                ["Memory", mb(report.totalMemMb)],
                ["Free disk", mb(report.freeDiskMb)],
                ["Web sites configured", report.sites.length ? report.sites.join(", ") : "none"],
              ]}
            />
          </Section>

          {blockers.length > 0 && (
            <Section title="Problems to fix">
              <ul className="flex flex-col gap-2">
                {blockers.map((c) => (
                  <li key={c.id}>
                    <Notice tone="error">{c.message}</Notice>
                  </li>
                ))}
              </ul>
            </Section>
          )}
          {warnings.length > 0 && (
            <Section title="Warnings">
              <ul className="flex flex-col gap-2">
                {warnings.map((c) => (
                  <li key={c.id}>
                    <Notice tone="warn">{c.message}</Notice>
                  </li>
                ))}
              </ul>
            </Section>
          )}

          <Section title="Package sources">
            {report.packageSources.length === 0 ? (
              <p className="text-sm text-slate-500">No package sources were reported.</p>
            ) : (
              <ul className="space-y-1 font-mono text-[13px] break-all">
                {report.packageSources.map((s, i) => (
                  <li key={i}>{s}</li>
                ))}
              </ul>
            )}
          </Section>
        </>
      )}
    </div>
  );
}
