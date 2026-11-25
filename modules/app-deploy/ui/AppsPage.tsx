import { AppDetail } from "./AppDetail";
import { AppList } from "./AppList";
import { CreateApp } from "./CreateApp";
import { OperationView } from "./OperationView";
import { useView } from "./store";

/** The Applications tab: list, create flow, app detail and operation progress. */
export function AppsPage({ serverId, connected }: { serverId: string; connected: boolean }) {
  const view = useView(serverId);
  switch (view.kind) {
    case "create":
      return <CreateApp serverId={serverId} connected={connected} />;
    case "detail":
      return <AppDetail key={view.appId} serverId={serverId} appId={view.appId} connected={connected} />;
    case "operation":
      return (
        <OperationView
          key={view.operationId}
          serverId={serverId}
          operationId={view.operationId}
          slug={view.slug}
          appId={view.appId}
          returnToList={view.returnToList === true}
        />
      );
    case "list":
      return <AppList serverId={serverId} connected={connected} />;
  }
}
