import { ServerWorkspace } from "@ossiframe/ui";
import { onConnected, onRemoved, onStale, pages } from "./modules";

export function App() {
  return (
    <ServerWorkspace
      title="{{product_name}}"
      pages={pages}
      onStale={onStale}
      onConnected={onConnected}
      onRemoved={onRemoved}
    />
  );
}
