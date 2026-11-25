import { useState } from "react";
import { appsApi, toIpcError, type AppDto } from "@ossiframe/client";
import { Button, Field, Input, Notice, errorMessage } from "@ossiframe/ui";
import type { Review } from "./AppDetail";
import { Section } from "./parts";

export function RemoveTab({ app, connected, onReview }: { app: AppDto; connected: boolean; onReview: Review }) {
  const [dropDb, setDropDb] = useState(false);
  const [delFiles, setDelFiles] = useState(false);
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const permanent = dropDb || delFiles;
  const confirmed = !permanent || typed === app.slug;

  async function remove() {
    setBusy(true);
    setError(null);
    try {
      const dto = await appsApi.removePlan(app.id, dropDb, delFiles);
      onReview(dto, {
        action: "Remove application",
        slug: app.slug,
        appId: app.id,
        rows: [
          ["Domain", app.domain],
          ["Database", dropDb ? "Dropped, with its user (permanent)" : app.hasDatabase ? "Kept" : "None"],
          ["Files and Linux user", delFiles ? "Deleted (permanent)" : "Kept"],
        ],
        creates: [],
        database: false,
        returnToList: true,
      });
    } catch (e) {
      setError(errorMessage(toIpcError(e)));
    }
    setBusy(false);
  }

  return (
    <Section title="Remove application">
      {error && <Notice tone="error">{error}</Notice>}
      <p className="text-sm">
        By default, removing stops the site and the PHP-FPM pool and deletes their configuration. The application's files, its Linux
        user and its database are kept, so nothing is lost. You can remove them too with the options below.
      </p>
      {app.hasDatabase ? (
        <label className="flex items-start gap-2 text-sm">
          <input type="checkbox" className="mt-1" checked={dropDb} onChange={(e) => setDropDb(e.target.checked)} />
          <span>Also drop the database and its user (permanent)</span>
        </label>
      ) : (
        <p className="text-xs text-slate-500">This application has no database.</p>
      )}
      <label className="flex items-start gap-2 text-sm">
        <input type="checkbox" className="mt-1" checked={delFiles} onChange={(e) => setDelFiles(e.target.checked)} />
        <span>Also delete the application files and Linux user (permanent)</span>
      </label>
      {permanent && (
        <>
          <Notice tone="warn">These options cannot be undone. Make sure you have a backup of anything you need.</Notice>
          <Field label={`Type ${app.slug} to confirm`}>
            {(p) => <Input {...p} value={typed} autoComplete="off" onChange={(e) => setTyped(e.target.value)} />}
          </Field>
        </>
      )}
      <div>
        <Button variant="destructive" disabled={!connected || busy || !confirmed} onClick={() => void remove()}>
          {busy ? "Preparing plan…" : "Remove application"}
        </Button>
        {!connected && <span className="ml-3 text-sm text-slate-500">Connect to the server first.</span>}
      </div>
    </Section>
  );
}
