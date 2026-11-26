import { useHosts, useStore } from "#ui/use-store.ts";
import { Back } from "#ui/app/back.tsx";
import type { FunctionComponent } from "react";
import { NewSession } from "#ui/session/new-session.tsx";
import { Route } from "@conductor/core/route";
import type { SessionId } from "@conductor/core/schemas/index";
import { useCallback } from "react";

interface Context {
  readonly directoryHost?: string;
  readonly project?: string;
  readonly issue?: string;
}

const NewSessionPane: FunctionComponent<Context> = (context) => {
  const store = useStore();
  const hosts = useHosts();
  const started = useCallback(
    (host: string, session: SessionId): void => {
      store.show(Route.Session({ host, session }));
    },
    [store],
  );
  return (
    <main className="pane">
      <Back />
      <NewSession
        directoryHost={context.directoryHost}
        issue={context.issue}
        project={context.project}
        hosts={hosts}
        send={store.send}
        started={started}
      />
    </main>
  );
};

export { NewSessionPane };
