import { ServerWorkspace } from "@ossiframe/ui";
import { clearServer, markStale, onConnected, pages } from "./modules";
export function App() {
  return (
    <ServerWorkspace
      title="Ossiframe Deploy"
      pages={pages}
      onStale={markStale}
      onConnected={onConnected}
      onRemoved={clearServer}
    />
  );
}
