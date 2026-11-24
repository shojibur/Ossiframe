import { useState, type ReactNode } from "react";
import { useServerConnections, type Server } from "@ossiframe/client";
import { Button } from "./button";
import { ConfirmRemoveDialog, CredentialDialog, HostKeyChangedDialog, HostKeyDialog } from "./dialogs";
import { errorMessage } from "./format";
import { ServerForm } from "./server-form";
import { StateBadge } from "./state-badge";
import { Mono, Notice } from "./status";

/** An optional module page shown as a tab. `id` equals the Rust module id. */
export interface ModuleUi {
  id: string;
  title: string;
  Page: (props: { serverId: string; connected: boolean }) => ReactNode;
}

export interface ServerWorkspaceProps {
  /** Application name shown above the server list. */
  title: string;
  /** Module pages shown as tabs. The tab bar is hidden when there is at most one. */
  pages?: ModuleUi[];
  /** Extra content rendered below the connection info and the active page. */
  children?: (ctx: { server: Server; connected: boolean }) => ReactNode;
  /** Data the app loaded for a server is out of date (disconnected, lost, edited). */
  onStale?: (serverId: string) => void;
  /** A connect attempt succeeded and the server list was refreshed. */
  onConnected?: (serverId: string) => void;
  /** A server profile was removed. */
  onRemoved?: (serverId: string) => void;
}

/**
 * Server list, connection controls, connection info and every connection dialog.
 * Works without any module page: it then shows only profile, endpoint, state and
 * the pinned host key fingerprint.
 */
export function ServerWorkspace({ title, pages = [], children, onStale, onConnected, onRemoved }: ServerWorkspaceProps) {
  const [selected, setSelected] = useState<string | null>(null);
  const [tab, setTab] = useState<string | null>(null);

  const c = useServerConnections({
    formatError: errorMessage,
    onStale,
    onConnected,
    onRemoved: (id) => {
      setSelected((s) => (s === id ? null : s));
      onRemoved?.(id);
    },
  });

  const { servers, dialog } = c;
  const server = servers.find((s) => s.id === selected) ?? null;
  const dialogServer = "serverId" in dialog ? servers.find((s) => s.id === dialog.serverId) : undefined;
  const connected = server?.state === "connected";
  const activePage = pages.find((p) => p.id === tab) ?? pages[0];

  return (
    <div className="flex h-screen text-slate-900 dark:bg-slate-950 dark:text-slate-100">
      <aside className="flex w-64 shrink-0 flex-col border-r border-slate-200 bg-slate-50 dark:border-slate-800 dark:bg-slate-900">
        <div className="flex items-center justify-between px-4 py-3">
          <span className="text-sm font-semibold">{title}</span>
          <Button size="sm" variant="secondary" onClick={() => c.openForm(null)}>
            Add server
          </Button>
        </div>
        <nav aria-label="Servers" className="flex flex-col overflow-y-auto px-2 pb-3">
          {servers.length === 0 && <p className="px-2 py-4 text-xs text-slate-500">No servers yet.</p>}
          {servers.map((s) => (
            <button
              key={s.id}
              type="button"
              onClick={() => setSelected(s.id)}
              aria-current={s.id === selected ? "page" : undefined}
              className="flex flex-col items-start gap-0.5 rounded-md px-2 py-2 text-left hover:bg-slate-200 aria-[current=page]:bg-slate-200 dark:hover:bg-slate-800 dark:aria-[current=page]:bg-slate-800"
            >
              <span className="text-sm font-medium">{s.name}</span>
              <span className="font-mono text-xs text-slate-500">{s.host}</span>
              <StateBadge server={s} busy={Boolean(c.busy[s.id])} />
            </button>
          ))}
        </nav>
      </aside>

      <main className="flex-1 overflow-y-auto">
        {c.globalError && (
          <div className="p-4">
            <Notice tone="error">{c.globalError}</Notice>
          </div>
        )}
        {server ? (
          <section className="flex flex-col gap-5 p-6">
            <header className="flex flex-wrap items-start justify-between gap-3">
              <div className="flex flex-col gap-1">
                <h1 className="text-lg font-semibold">{server.name}</h1>
                <Mono className="text-slate-600 dark:text-slate-400">
                  {server.username}@{server.host}:{server.port}
                </Mono>
                <StateBadge server={server} busy={Boolean(c.busy[server.id])} />
              </div>
              <div className="flex flex-wrap gap-2">
                {connected ? (
                  <Button variant="secondary" onClick={() => void c.disconnect(server.id)}>
                    Disconnect
                  </Button>
                ) : (
                  <Button onClick={() => void c.connect(server.id)} disabled={Boolean(c.busy[server.id])}>
                    Connect
                  </Button>
                )}
                <Button variant="ghost" onClick={() => c.openForm(server)}>
                  Edit
                </Button>
                <Button variant="ghost" onClick={() => c.openRemove(server.id)}>
                  Remove
                </Button>
              </div>
            </header>

            {c.errors[server.id] && <Notice tone="error">{c.errors[server.id]}</Notice>}
            {c.warnings[server.id] && <Notice tone="warn">{c.warnings[server.id]}</Notice>}

            <div className="text-xs text-slate-500">
              Pinned host key:{" "}
              {server.pinnedHostKey ? (
                <Mono>
                  {server.pinnedHostKey.keyType} {server.pinnedHostKey.sha256}
                </Mono>
              ) : (
                "none yet (verified on first connection)"
              )}
            </div>

            {pages.length > 1 && (
              <div role="tablist" aria-label="Pages" className="flex gap-1 border-b border-slate-200 dark:border-slate-800">
                {pages.map((p) => (
                  <button
                    key={p.id}
                    type="button"
                    role="tab"
                    aria-selected={p.id === activePage?.id}
                    onClick={() => setTab(p.id)}
                    className="-mb-px border-b-2 border-transparent px-3 py-1.5 text-sm text-slate-600 hover:text-slate-900 aria-selected:border-slate-900 aria-selected:text-slate-900 dark:text-slate-400 dark:hover:text-slate-100 dark:aria-selected:border-slate-100 dark:aria-selected:text-slate-100"
                  >
                    {p.title}
                  </button>
                ))}
              </div>
            )}
            {activePage && <activePage.Page key={`${server.id}:${activePage.id}`} serverId={server.id} connected={connected} />}
            {children?.({ server, connected })}
          </section>
        ) : (
          <div className="flex h-full items-center justify-center text-sm text-slate-500">
            {servers.length ? "Select a server." : "Add a server you are authorized to access to get started."}
          </div>
        )}
      </main>

      {dialog.type === "form" && (
        <ServerForm
          key={dialog.server?.id ?? "new"}
          open
          server={dialog.server}
          onClose={() => c.closeDialog()}
          onSaved={(s) => {
            setSelected(s.id);
            c.profileSaved(s);
          }}
        />
      )}
      {dialog.type === "credential" && dialogServer && (
        <CredentialDialog
          server={dialogServer}
          kind={dialog.kind}
          error={dialog.error}
          onCancel={() => c.closeDialog()}
          onSubmit={(cred, save) => void c.connect(dialog.serverId, cred, save)}
        />
      )}
      {dialog.type === "hostKey" && dialogServer && (
        <HostKeyDialog
          server={dialogServer}
          keyType={dialog.keyType}
          sha256={dialog.sha256}
          onCancel={c.closeDialog}
          onTrust={() => void c.trust(dialog.serverId, dialog.token)}
        />
      )}
      {dialog.type === "changed" && dialogServer && (
        <HostKeyChangedDialog
          server={dialogServer}
          pinned={dialog.pinned}
          presented={dialog.presented}
          error={dialog.error}
          onClose={() => c.closeDialog()}
          onReplace={(h) => void c.replace(dialog.serverId, dialog.token, h)}
        />
      )}
      {dialog.type === "remove" && dialogServer && (
        <ConfirmRemoveDialog
          server={dialogServer}
          onCancel={() => c.closeDialog()}
          onConfirm={() => void c.remove(dialog.serverId)}
        />
      )}
    </div>
  );
}
