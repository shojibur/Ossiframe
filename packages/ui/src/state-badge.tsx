import type { Server } from "@ossiframe/client";
import { StatusBadge, type Tone } from "./status";

const stateLabel: Record<Server["state"], [Tone, string]> = {
  connected: ["ok", "Connected"],
  connecting: ["busy", "Connecting"],
  disconnected: ["neutral", "Disconnected"],
  error: ["error", "Error"],
};

export function StateBadge({ server, busy }: { server: Server; busy: boolean }) {
  const [tone, label] = busy ? stateLabel.connecting : stateLabel[server.state];
  return <StatusBadge tone={tone}>{label}</StatusBadge>;
}
