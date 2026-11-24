import { useState, type FormEvent } from "react";
import type { CredentialKind, Server } from "@ossiframe/client";
import { Button, Dialog, Field, Input, Mono, Notice } from "@ossiframe/ui";

export function CredentialDialog(props: {
  server: Server;
  kind: CredentialKind;
  error: string | null;
  onSubmit: (credential: string, save: boolean) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState("");
  const [save, setSave] = useState(false);
  const label = props.kind === "password" ? "Password" : "Key passphrase";

  function submit(e: FormEvent) {
    e.preventDefault();
    const v = value;
    setValue(""); // Do not keep the credential in UI state after submitting.
    props.onSubmit(v, save);
  }

  return (
    <Dialog
      open
      onOpenChange={(o) => !o && props.onCancel()}
      title={`Authenticate to ${props.server.name}`}
      description={`${props.server.username}@${props.server.host}:${props.server.port}`}
    >
      <form onSubmit={submit} className="flex flex-col gap-3">
        {props.error && <Notice tone="error">{props.error}</Notice>}
        <Field label={label}>
          {(p) => <Input {...p} type="password" autoFocus autoComplete="off" value={value} onChange={(e) => setValue(e.target.value)} />}
        </Field>
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={save} onChange={(e) => setSave(e.target.checked)} />
          Save in the OS keychain after a successful login
        </label>
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={props.onCancel}>
            Cancel
          </Button>
          <Button type="submit" disabled={value.length === 0}>
            Connect
          </Button>
        </div>
      </form>
    </Dialog>
  );
}

export function HostKeyDialog(props: {
  server: Server;
  keyType: string;
  sha256: string;
  onTrust: () => void;
  onCancel: () => void;
}) {
  return (
    <Dialog
      open
      onOpenChange={(o) => !o && props.onCancel()}
      title="Verify this server's fingerprint"
      description="This is the first connection to this server, so its identity has not been verified. Nothing has been sent to it yet."
      footer={
        <>
          <Button variant="secondary" onClick={props.onCancel}>
            Cancel
          </Button>
          <Button onClick={props.onTrust}>Trust and connect</Button>
        </>
      }
    >
      <div className="flex flex-col gap-3 text-sm">
        <div>
          <div className="text-xs text-slate-500">Server</div>
          <Mono>
            {props.server.host}:{props.server.port}
          </Mono>
        </div>
        <div>
          <div className="text-xs text-slate-500">Key type</div>
          <Mono>{props.keyType}</Mono>
        </div>
        <div>
          <div className="text-xs text-slate-500">SHA-256 fingerprint</div>
          <Mono className="block rounded bg-slate-100 p-2 dark:bg-slate-800">{props.sha256}</Mono>
        </div>
        <p className="text-slate-600 dark:text-slate-400">
          Compare it with a source you trust, such as your provider's console or <Mono>ssh-keygen -lf /etc/ssh/ssh_host_{props.keyType.replace("ssh-", "")}_key.pub</Mono> run on the server. Trusting pins this key; future connections fail if it changes.
        </p>
      </div>
    </Dialog>
  );
}

export function HostKeyChangedDialog(props: {
  server: Server;
  pinned: { keyType: string; sha256: string };
  presented: { keyType: string; sha256: string };
  error: string | null;
  onReplace: (confirmHost: string) => void;
  onClose: () => void;
}) {
  const [reviewing, setReviewing] = useState(false);
  const [typed, setTyped] = useState("");
  return (
    <Dialog
      open
      onOpenChange={(o) => !o && props.onClose()}
      title="Host key changed"
      description="The server presented a different host key than the one pinned for it. The connection was blocked before any credential was sent."
      footer={
        reviewing ? (
          <>
            <Button variant="secondary" onClick={props.onClose}>
              Cancel
            </Button>
            <Button variant="destructive" disabled={typed.trim().toLowerCase() !== props.server.host} onClick={() => props.onReplace(typed)}>
              Replace pinned key
            </Button>
          </>
        ) : (
          <>
            <Button variant="secondary" onClick={() => setReviewing(true)}>
              Review replacement
            </Button>
            <Button onClick={props.onClose}>Close</Button>
          </>
        )
      }
    >
      <div className="flex flex-col gap-3 text-sm">
        <Notice tone="error">
          This can mean the server was reinstalled, or that someone is intercepting the connection. Only replace the key if you know why it changed.
        </Notice>
        <div>
          <div className="text-xs text-slate-500">Pinned ({props.pinned.keyType})</div>
          <Mono className="block rounded bg-slate-100 p-2 dark:bg-slate-800">{props.pinned.sha256}</Mono>
        </div>
        <div>
          <div className="text-xs text-slate-500">Presented now ({props.presented.keyType})</div>
          <Mono className="block rounded bg-slate-100 p-2 dark:bg-slate-800">{props.presented.sha256}</Mono>
        </div>
        {reviewing && (
          <Field label={`Type the host name (${props.server.host}) to confirm`} error={props.error ?? undefined}>
            {(p) => <Input {...p} value={typed} onChange={(e) => setTyped(e.target.value)} autoFocus spellCheck={false} />}
          </Field>
        )}
      </div>
    </Dialog>
  );
}

export function ConfirmRemoveDialog(props: { server: Server; onConfirm: () => void; onCancel: () => void }) {
  return (
    <Dialog
      open
      onOpenChange={(o) => !o && props.onCancel()}
      title={`Remove ${props.server.name}?`}
      description="This removes the local profile, its pinned host key and any saved secret from this computer. The server itself is not contacted or changed."
      footer={
        <>
          <Button variant="secondary" onClick={props.onCancel}>
            Cancel
          </Button>
          <Button variant="destructive" onClick={props.onConfirm}>
            Remove
          </Button>
        </>
      }
    >
      {null}
    </Dialog>
  );
}
