import { useState, type FormEvent } from "react";
import { api, type AuthKind, type IpcError, type ProfileInput, type Server } from "@ossiframe/client";
import { Button, Dialog, Field, Input, Notice } from "@ossiframe/ui";
import { errorMessage } from "./format";

interface Props {
  open: boolean;
  server: Server | null;
  onClose: () => void;
  onSaved: (s: Server) => void;
}

const empty: ProfileInput = { name: "", host: "", port: 22, username: "", authKind: "keyFile", keyPath: "~/.ssh/id_ed25519" };

export function ServerForm({ open, server, onClose, onSaved }: Props) {
  const [form, setForm] = useState<ProfileInput>(() =>
    server
      ? { name: server.name, host: server.host, port: server.port, username: server.username, authKind: server.authKind, keyPath: server.keyPath }
      : empty,
  );
  const [error, setError] = useState<IpcError | null>(null);
  const [busy, setBusy] = useState(false);

  const fieldError = (f: string) => (error?.kind === "validation" && error.field === f ? error.message : undefined);
  const set = <K extends keyof ProfileInput>(k: K, v: ProfileInput[K]) => setForm((p) => ({ ...p, [k]: v }));
  const endpointChanged = server !== null && (form.host.trim().toLowerCase() !== server.host || form.port !== server.port);
  const authChanged =
    server !== null && (form.authKind !== server.authKind || form.username.trim() !== server.username || (form.authKind === "keyFile" && form.keyPath !== server.keyPath));

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    const input = { ...form, keyPath: form.authKind === "keyFile" ? form.keyPath : null };
    try {
      onSaved(server ? await api.updateServer(server.id, input) : await api.addServer(input));
    } catch (err) {
      setError(err as IpcError);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => !o && onClose()}
      title={server ? "Edit server" : "Add server"}
      description="Only this computer stores the profile. Nothing is changed on the server."
    >
      <form onSubmit={submit} className="flex flex-col gap-3" noValidate>
        <Field label="Name" error={fieldError("name")}>
          {(p) => <Input {...p} value={form.name} onChange={(e) => set("name", e.target.value)} autoFocus />}
        </Field>
        <div className="grid grid-cols-[1fr_96px] gap-3">
          <Field label="Host (IP or DNS name)" error={fieldError("host")}>
            {(p) => <Input {...p} value={form.host} onChange={(e) => set("host", e.target.value)} spellCheck={false} autoCapitalize="off" />}
          </Field>
          <Field label="Port" error={fieldError("port")}>
            {(p) => <Input {...p} type="number" min={1} max={65535} value={form.port} onChange={(e) => set("port", Number(e.target.value))} />}
          </Field>
        </div>
        <Field label="Username" error={fieldError("username")}>
          {(p) => <Input {...p} value={form.username} onChange={(e) => set("username", e.target.value)} spellCheck={false} autoCapitalize="off" />}
        </Field>
        <fieldset className="flex gap-4 text-sm">
          <legend className="mb-1 text-xs font-medium text-slate-700 dark:text-slate-300">Authentication</legend>
          {(["keyFile", "password"] as AuthKind[]).map((k) => (
            <label key={k} className="flex items-center gap-1.5">
              <input type="radio" name="auth" checked={form.authKind === k} onChange={() => set("authKind", k)} />
              {k === "keyFile" ? "Private key file" : "Password"}
            </label>
          ))}
        </fieldset>
        {form.authKind === "keyFile" && (
          <Field label="Private key path" error={fieldError("keyPath")} hint="Absolute path or ~/… The key file stays on this computer.">
            {(p) => <Input {...p} value={form.keyPath ?? ""} onChange={(e) => set("keyPath", e.target.value)} spellCheck={false} className="font-mono" />}
          </Field>
        )}
        <p className="text-xs text-slate-500">You are asked for a password or key passphrase when connecting. Saving it to the OS keychain is optional.</p>
        {endpointChanged && <Notice tone="warn">Changing the host or port disconnects the server and removes its pinned host key. You must verify the new fingerprint.</Notice>}
        {authChanged && server?.hasSavedSecret && <Notice tone="warn">Changing authentication deletes the saved secret for this server.</Notice>}
        {error && error.kind !== "validation" && <Notice tone="error">{errorMessage(error)}</Notice>}
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" disabled={busy}>
            {server ? "Save" : "Add server"}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
