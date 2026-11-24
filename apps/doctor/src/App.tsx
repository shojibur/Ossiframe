import { useCallback, useEffect, useRef, useState } from "react";
import {
  api,
  onConnectionLost,
  onConnectionState,
  toIpcError,
  type CredentialKind,
  type IpcError,
  type Server,
} from "@ossiframe/client";
import { Button, Notice } from "@ossiframe/ui";
import { ConfirmRemoveDialog, CredentialDialog, HostKeyChangedDialog, HostKeyDialog } from "./dialogs";
import { errorMessage } from "./format";
import { Overview, StateBadge, type InfoState } from "./Overview";
import { ServerForm } from "./ServerForm";

type DialogState =
  | { type: "none" }
  | { type: "form"; server: Server | null }
  | { type: "credential"; serverId: string; kind: CredentialKind; error: string | null }
  | { type: "hostKey"; serverId: string; token: string; keyType: string; sha256: string }
  | {
      type: "changed";
      serverId: string;
      token: string;
      pinned: { keyType: string; sha256: string };
      presented: { keyType: string; sha256: string };
      error: string | null;
    }
  | { type: "remove"; serverId: string };

export function App() {
  const [servers, setServers] = useState<Server[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [infos, setInfos] = useState<Record<string, InfoState>>({});
  const [loadingInfo, setLoadingInfo] = useState<Record<string, boolean>>({});
  const [busy, setBusy] = useState<Record<string, boolean>>({});
  const [errors, setErrors] = useState<Record<string, string | null>>({});
  const [globalError, setGlobalError] = useState<string | null>(null);
  const [dialog, setDialog] = useState<DialogState>({ type: "none" });
  // A credential entered before a host key prompt, kept only until the retry.
  const pending = useRef<{ credential: string; save: boolean } | null>(null);

  const setErr = (id: string, msg: string | null) => setErrors((e) => ({ ...e, [id]: msg }));
  const markStale = (id: string) =>
    setInfos((m) => (m[id] ? { ...m, [id]: { info: m[id].info, stale: true } } : m));

  const refresh = useCallback(async () => {
    try {
      const list = await api.listServers();
      setServers(list);
      setGlobalError(null);
      for (const s of list) if (s.state !== "connected") markStale(s.id);
    } catch (e) {
      setGlobalError(errorMessage(toIpcError(e)));
    }
  }, []);

  useEffect(() => {
    const first = window.setTimeout(() => void refresh(), 0);
    const unlisten = [
      onConnectionState(() => void refresh()),
      onConnectionLost((id) => {
        markStale(id);
        setErr(id, errorMessage({ kind: "connectionLost" }));
        void refresh();
      }),
    ];
    // Catch sessions that died while idle (keepalive timeout).
    const timer = window.setInterval(() => void refresh(), 15000);
    return () => {
      window.clearTimeout(first);
      window.clearInterval(timer);
      unlisten.forEach((p) => void p.then((f) => f()));
    };
  }, [refresh]);

  const server = servers.find((s) => s.id === selected) ?? null;
  const byId = (id: string) => servers.find((s) => s.id === id);

  async function loadInfo(id: string) {
    setLoadingInfo((m) => ({ ...m, [id]: true }));
    setErr(id, null);
    try {
      const info = await api.getServerInfo(id);
      setInfos((m) => ({ ...m, [id]: { info, stale: false } }));
    } catch (e) {
      const err = toIpcError(e);
      if (err.kind === "connectionLost" || err.kind === "notConnected") markStale(id);
      setErr(id, errorMessage(err));
    } finally {
      setLoadingInfo((m) => ({ ...m, [id]: false }));
      void refresh();
    }
  }

  async function connect(id: string, credential: string | null = null, save = false) {
    setBusy((m) => ({ ...m, [id]: true }));
    setErr(id, null);
    try {
      const r = await api.connectServer(id, credential, save);
      if (r.status === "connected") {
        setDialog({ type: "none" });
        await refresh();
        void loadInfo(id);
      } else {
        pending.current = credential ? { credential, save } : null;
        setDialog({ type: "hostKey", serverId: id, token: r.token, keyType: r.keyType, sha256: r.sha256 });
      }
    } catch (e) {
      handleConnectError(id, toIpcError(e), credential !== null);
    } finally {
      setBusy((m) => ({ ...m, [id]: false }));
      void refresh();
    }
  }

  function handleConnectError(id: string, err: IpcError, usedCredential: boolean) {
    const s = byId(id);
    if (err.kind === "credentialRequired") {
      setDialog({ type: "credential", serverId: id, kind: err.credential, error: null });
    } else if (err.kind === "authFailed" && s && (s.authKind === "password" || usedCredential || s.hasSavedSecret)) {
      const kind: CredentialKind = s.authKind === "password" ? "password" : "passphrase";
      setDialog({ type: "credential", serverId: id, kind, error: "Authentication failed. Enter the credential again." });
    } else if (err.kind === "hostKeyMismatch") {
      setDialog({
        type: "changed",
        serverId: id,
        token: err.replaceToken,
        pinned: { keyType: err.pinnedKeyType, sha256: err.pinnedSha256 },
        presented: { keyType: err.presentedKeyType, sha256: err.presentedSha256 },
        error: null,
      });
    } else {
      setDialog({ type: "none" });
      setErr(id, errorMessage(err));
    }
  }

  async function trust(id: string, token: string) {
    setDialog({ type: "none" });
    try {
      await api.trustHostKey(id, token);
    } catch (e) {
      pending.current = null;
      setErr(id, errorMessage(toIpcError(e)));
      return;
    }
    const p = pending.current;
    pending.current = null;
    await connect(id, p?.credential ?? null, p?.save ?? false);
  }

  async function replace(id: string, token: string, confirmHost: string) {
    try {
      await api.replaceHostKey(id, token, confirmHost);
    } catch (e) {
      const err = toIpcError(e);
      if (err.kind === "confirmationMismatch") {
        setDialog((d) => (d.type === "changed" ? { ...d, error: errorMessage(err) } : d));
        return;
      }
      setDialog({ type: "none" });
      setErr(id, errorMessage(err));
      return;
    }
    setDialog({ type: "none" });
    await refresh();
    await connect(id);
  }

  async function disconnect(id: string) {
    try {
      await api.disconnectServer(id);
    } catch (e) {
      setErr(id, errorMessage(toIpcError(e)));
    }
    markStale(id);
    await refresh();
  }

  async function remove(id: string) {
    setDialog({ type: "none" });
    try {
      await api.removeServer(id);
      setInfos((m) => {
        const rest = { ...m };
        delete rest[id];
        return rest;
      });
      if (selected === id) setSelected(null);
    } catch (e) {
      setErr(id, errorMessage(toIpcError(e)));
    }
    await refresh();
  }

  const dialogServer = "serverId" in dialog ? byId(dialog.serverId) : undefined;

  return (
    <div className="flex h-screen text-slate-900 dark:bg-slate-950 dark:text-slate-100">
      <aside className="flex w-64 shrink-0 flex-col border-r border-slate-200 bg-slate-50 dark:border-slate-800 dark:bg-slate-900">
        <div className="flex items-center justify-between px-4 py-3">
          <span className="text-sm font-semibold">Ossiframe Doctor</span>
          <Button size="sm" variant="secondary" onClick={() => setDialog({ type: "form", server: null })}>
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
              <StateBadge server={s} busy={Boolean(busy[s.id])} />
            </button>
          ))}
        </nav>
      </aside>

      <main className="flex-1 overflow-y-auto">
        {globalError && (
          <div className="p-4">
            <Notice tone="error">{globalError}</Notice>
          </div>
        )}
        {server ? (
          <Overview
            server={server}
            info={infos[server.id]}
            busy={Boolean(busy[server.id])}
            loadingInfo={Boolean(loadingInfo[server.id])}
            error={errors[server.id] ?? null}
            onConnect={() => void connect(server.id)}
            onDisconnect={() => void disconnect(server.id)}
            onRefresh={() => void loadInfo(server.id)}
            onCancel={() => void api.cancelOperation(server.id)}
            onEdit={() => setDialog({ type: "form", server })}
            onRemove={() => setDialog({ type: "remove", serverId: server.id })}
          />
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
          onClose={() => setDialog({ type: "none" })}
          onSaved={(s) => {
            setDialog({ type: "none" });
            setSelected(s.id);
            if (s.state !== "connected") markStale(s.id);
            void refresh();
          }}
        />
      )}
      {dialog.type === "credential" && dialogServer && (
        <CredentialDialog
          server={dialogServer}
          kind={dialog.kind}
          error={dialog.error}
          onCancel={() => setDialog({ type: "none" })}
          onSubmit={(c, save) => void connect(dialog.serverId, c, save)}
        />
      )}
      {dialog.type === "hostKey" && dialogServer && (
        <HostKeyDialog
          server={dialogServer}
          keyType={dialog.keyType}
          sha256={dialog.sha256}
          onCancel={() => {
            pending.current = null;
            setDialog({ type: "none" });
          }}
          onTrust={() => void trust(dialog.serverId, dialog.token)}
        />
      )}
      {dialog.type === "changed" && dialogServer && (
        <HostKeyChangedDialog
          server={dialogServer}
          pinned={dialog.pinned}
          presented={dialog.presented}
          error={dialog.error}
          onClose={() => setDialog({ type: "none" })}
          onReplace={(h) => void replace(dialog.serverId, dialog.token, h)}
        />
      )}
      {dialog.type === "remove" && dialogServer && (
        <ConfirmRemoveDialog
          server={dialogServer}
          onCancel={() => setDialog({ type: "none" })}
          onConfirm={() => void remove(dialog.serverId)}
        />
      )}
    </div>
  );
}
