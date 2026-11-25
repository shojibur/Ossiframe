import { useEffect, useState } from "react";
import { Button, Mono, Notice, errorMessage, localTime } from "@ossiframe/ui";
import { OperationStateBadge, Table, Td } from "./parts";
import { loadHistory, showOperation, useServerEntry } from "./store";

/** Past operations for this server, from local storage. Survives profile removal. */
export function HistoryPage({ serverId }: { serverId: string; connected: boolean }) {
  const entry = useServerEntry(serverId);
  const [open, setOpen] = useState<string | null>(null);

  useEffect(() => {
    void loadHistory(serverId);
  }, [serverId]);

  const detail = entry.operation && entry.operation.id === open ? entry.operation : null;

  return (
    <div className="flex flex-col gap-4">
      {entry.error && <Notice tone="error">{errorMessage(entry.error)}</Notice>}
      {entry.history.length === 0 ? (
        <p className="text-sm text-slate-500">No operations yet. Plans you approve are listed here.</p>
      ) : (
        <Table head={["Created", "State", "Finished", "Error", ""]}>
          {entry.history.map((h) => (
            <tr key={h.id}>
              <Td>{localTime(h.createdAt)}</Td>
              <Td>
                <OperationStateBadge state={h.state} />
              </Td>
              <Td>{h.finishedAt ? localTime(h.finishedAt) : "Not finished"}</Td>
              <Td>{h.error ?? ""}</Td>
              <Td>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => {
                    setOpen(h.id);
                    showOperation(serverId, h.id);
                  }}
                >
                  Details
                </Button>
              </Td>
            </tr>
          ))}
        </Table>
      )}

      {detail && (
        <div className="flex flex-col gap-3 rounded-lg border border-slate-200 p-4 text-sm dark:border-slate-800">
          <h2 className="text-sm font-semibold">Recorded details</h2>
          <div className="flex flex-col gap-1">
            <span className="text-xs text-slate-500">Plan digest (SHA-256)</span>
            <Mono>{detail.planDigest}</Mono>
          </div>
          <p>
            Server at the time: {detail.server.name} ({detail.server.user}@{detail.server.host}:{detail.server.port})
            {detail.serverId === null && " (the server profile was removed)"}
          </p>
          {detail.resolution && (
            <p>
              Closed after inspection on {localTime(detail.resolution.decidedAt)}: {detail.resolution.decision}
            </p>
          )}
          {detail.events.length > 0 && (
            <ul className="space-y-1">
              {detail.events.map((e, i) => (
                <li key={i} className="text-xs">
                  <span className="text-slate-500">{localTime(e.at)}</span> {e.kind}
                  {e.fromState && e.toState ? `: ${e.fromState} to ${e.toState}` : ""}
                  {e.unitName ? ` (${e.unitName})` : ""}
                  {e.detail ? `. ${e.detail}` : ""}
                </li>
              ))}
            </ul>
          )}
          <p className="text-xs text-slate-500">Open the Operation page for steps, logs and verification.</p>
        </div>
      )}
    </div>
  );
}
