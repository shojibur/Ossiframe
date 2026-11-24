// Connection flow shared by Ossiframe apps: server list, connect, host key
// trust and replace, credential prompts, disconnect and remove. No JSX.
import { useCallback, useEffect, useRef, useState } from "react";
import { api, onConnectionLost, onConnectionState, toIpcError } from "./api";
import type { CredentialKind, IpcError, Server } from "./types";

export type DialogState =
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

export interface ServerConnectionsOptions {
  /** Turns an IPC error into the text shown to the user. */
  formatError: (e: IpcError) => string;
  /** Server data loaded by the app is out of date (disconnected, lost, edited). */
  onStale?: (serverId: string) => void;
  /** A connect attempt succeeded and the server list was refreshed. */
  onConnected?: (serverId: string) => void;
  /** The profile was removed. */
  onRemoved?: (serverId: string) => void;
}

export interface ServerConnections {
  servers: Server[];
  busy: Record<string, boolean>;
  errors: Record<string, string | null>;
  warnings: Record<string, string | null>;
  globalError: string | null;
  dialog: DialogState;
  refresh: () => Promise<void>;
  setError: (serverId: string, message: string | null) => void;
  /** Opens the add/edit form (`null` adds) or the remove confirmation. */
  openForm: (server: Server | null) => void;
  openRemove: (serverId: string) => void;
  closeDialog: () => void;
  /** Call after the profile form saved a profile. */
  profileSaved: (server: Server) => void;
  connect: (id: string, credential?: string | null, save?: boolean) => Promise<void>;
  trust: (id: string, token: string) => Promise<void>;
  replace: (id: string, token: string, confirmHost: string) => Promise<void>;
  disconnect: (id: string) => Promise<void>;
  remove: (id: string) => Promise<void>;
}

export function useServerConnections(options: ServerConnectionsOptions): ServerConnections {
  const [servers, setServers] = useState<Server[]>([]);
  const [busy, setBusy] = useState<Record<string, boolean>>({});
  const [errors, setErrors] = useState<Record<string, string | null>>({});
  const [warnings, setWarnings] = useState<Record<string, string | null>>({});
  const [globalError, setGlobalError] = useState<string | null>(null);
  const [dialog, setDialog] = useState<DialogState>({ type: "none" });
  // A credential entered before a host key prompt. It belongs to exactly one
  // server and one trust token, lives only in memory, and is cleared whenever
  // the flow ends, is abandoned or another attempt starts.
  const pending = useRef<{ serverId: string; token: string; credential: string; save: boolean } | null>(null);

  // Latest callbacks, so `refresh` and the listeners stay stable.
  const opts = useRef(options);
  useEffect(() => {
    opts.current = options;
  });
  const fmt = (e: IpcError) => opts.current.formatError(e);

  const closeDialog = () => {
    pending.current = null;
    setDialog({ type: "none" });
  };
  const setErr = (id: string, msg: string | null) => setErrors((e) => ({ ...e, [id]: msg }));
  const markStale = (id: string) => opts.current.onStale?.(id);

  const refresh = useCallback(async () => {
    try {
      const list = await api.listServers();
      setServers(list);
      setGlobalError(null);
      for (const s of list) if (s.state !== "connected") opts.current.onStale?.(s.id);
    } catch (e) {
      setGlobalError(opts.current.formatError(toIpcError(e)));
    }
  }, []);

  useEffect(() => {
    const first = window.setTimeout(() => void refresh(), 0);
    const unlisten = [
      onConnectionState(() => void refresh()),
      onConnectionLost((id) => {
        opts.current.onStale?.(id);
        setErrors((e) => ({ ...e, [id]: opts.current.formatError({ kind: "connectionLost" }) }));
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

  const byId = (id: string) => servers.find((s) => s.id === id);

  async function connect(id: string, credential: string | null = null, save = false) {
    pending.current = null; // superseded by this attempt
    setBusy((m) => ({ ...m, [id]: true }));
    setErr(id, null);
    setWarnings((w) => ({ ...w, [id]: null }));
    try {
      const r = await api.connectServer(id, credential, save);
      if (r.status === "connected") {
        closeDialog();
        setWarnings((w) => ({ ...w, [id]: r.warning }));
        await refresh();
        opts.current.onConnected?.(id);
      } else {
        pending.current = credential ? { serverId: id, token: r.token, credential, save } : null;
        setDialog({ type: "hostKey", serverId: id, token: r.token, keyType: r.keyType, sha256: r.sha256 });
      }
    } catch (e) {
      pending.current = null;
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
      closeDialog();
      setErr(id, fmt(err));
    }
  }

  async function trust(id: string, token: string) {
    const held = pending.current;
    const p = held && held.serverId === id && held.token === token ? held : null;
    closeDialog();
    try {
      await api.trustHostKey(id, token);
    } catch (e) {
      setErr(id, fmt(toIpcError(e)));
      return;
    }
    await connect(id, p?.credential ?? null, p?.save ?? false);
  }

  async function replace(id: string, token: string, confirmHost: string) {
    try {
      await api.replaceHostKey(id, token, confirmHost);
    } catch (e) {
      const err = toIpcError(e);
      if (err.kind === "confirmationMismatch") {
        setDialog((d) => (d.type === "changed" ? { ...d, error: fmt(err) } : d));
        return;
      }
      closeDialog();
      setErr(id, fmt(err));
      return;
    }
    closeDialog();
    await refresh();
    await connect(id);
  }

  async function disconnect(id: string) {
    try {
      await api.disconnectServer(id);
    } catch (e) {
      setErr(id, fmt(toIpcError(e)));
    }
    markStale(id);
    await refresh();
  }

  async function remove(id: string) {
    closeDialog();
    try {
      await api.removeServer(id);
      opts.current.onRemoved?.(id);
    } catch (e) {
      setErr(id, fmt(toIpcError(e)));
    }
    await refresh();
  }

  return {
    servers,
    busy,
    errors,
    warnings,
    globalError,
    dialog,
    refresh,
    setError: setErr,
    openForm: (server) => {
      setDialog({ type: "form", server });
    },
    openRemove: (serverId) => {
      setDialog({ type: "remove", serverId });
    },
    closeDialog,
    profileSaved: (server) => {
      closeDialog();
      if (server.state !== "connected") markStale(server.id);
      void refresh();
    },
    connect,
    trust,
    replace,
    disconnect,
    remove,
  };
}
