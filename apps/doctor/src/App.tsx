import { ServerWorkspace } from "@ossiframe/ui";
import { clearServer, loadServerInfo, markStale, pages } from "./modules";

export function App() {
  return (
    <ServerWorkspace
      title="Ossiframe Doctor"
      pages={pages}
      onStale={markStale}
      onConnected={(id) => void loadServerInfo(id)}
      onRemoved={clearServer}
    />
  );
}
