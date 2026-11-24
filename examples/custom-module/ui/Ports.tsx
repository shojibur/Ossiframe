import { useState } from "react";
import { api, toIpcError } from "@ossiframe/client";
import { Button, Notice, errorMessage, localTime } from "@ossiframe/ui";

type Family = "ipv4" | "ipv6" | "all";

/** Result of the `ports` / `listening-tcp` operation. */
interface ListeningTcp {
  observedAt: string;
  sockets: { localAddress: string; port: number; process: string | null }[];
  skipped: number;
  truncated: boolean;
}

const FAMILIES: { value: Family; label: string }[] = [
  { value: "all", label: "All" },
  { value: "ipv4", label: "IPv4" },
  { value: "ipv6", label: "IPv6" },
];

export function Ports({ serverId, connected }: { serverId: string; connected: boolean }) {
  const [family, setFamily] = useState<Family>("all");
  const [result, setResult] = useState<ListeningTcp | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function load() {
    setLoading(true);
    setError(null);
    try {
      setResult(await api.runOperation<ListeningTcp>(serverId, "ports", "listening-tcp", { family }));
    } catch (e) {
      setError(errorMessage(toIpcError(e)));
    } finally {
      setLoading(false);
    }
  }

  return (
    <div className="flex flex-col gap-5">
      {connected && (
        <div className="flex items-center gap-2">
          <label className="text-sm" htmlFor="ports-family">
            Family
          </label>
          <select
            id="ports-family"
            className="rounded-md border border-slate-300 bg-transparent px-2 py-1 text-sm dark:border-slate-700"
            value={family}
            disabled={loading}
            onChange={(e) => setFamily(e.target.value as Family)}
          >
            {FAMILIES.map((f) => (
              <option key={f.value} value={f.value}>
                {f.label}
              </option>
            ))}
          </select>
          {loading ? (
            <Button variant="secondary" onClick={() => void api.cancelOperation(serverId)}>
              Cancel
            </Button>
          ) : (
            <Button onClick={() => void load()}>{result ? "Refresh" : "Load"}</Button>
          )}
        </div>
      )}

      {error && <Notice tone="error">{error}</Notice>}
      {!connected && !result && <p className="text-sm text-slate-500">Connect to list listening TCP ports.</p>}

      {result && (
        <div className="flex flex-col gap-3 rounded-lg border border-slate-200 p-4 dark:border-slate-800">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <h2 className="text-sm font-semibold">Listening TCP sockets</h2>
            <span className="text-xs text-slate-500">Observed {localTime(result.observedAt)}</span>
          </div>
          {!connected && <Notice tone="warn">Not connected. This list may be out of date.</Notice>}
          {result.truncated && <Notice tone="warn">The list was truncated. Not all sockets are shown.</Notice>}
          {result.skipped > 0 && <p className="text-xs text-slate-500">{result.skipped} unparsable line(s) skipped.</p>}
          {result.sockets.length === 0 ? (
            <p className="text-sm text-slate-500">No listening sockets found.</p>
          ) : (
            <table className="w-full text-left text-sm">
              <thead>
                <tr className="border-b border-slate-200 text-xs text-slate-500 dark:border-slate-800">
                  <th className="py-1 pr-4 font-medium">Address</th>
                  <th className="py-1 font-medium">Port</th>
                </tr>
              </thead>
              <tbody>
                {result.sockets.map((s, i) => (
                  <tr key={`${s.localAddress}:${s.port}:${i}`} className="border-b border-slate-100 dark:border-slate-900">
                    <td className="py-1 pr-4 font-mono">{s.localAddress}</td>
                    <td className="py-1 font-mono">{s.port}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      )}
    </div>
  );
}
