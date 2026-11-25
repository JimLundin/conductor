import type { Command, SessionId } from "@conductor/core/schemas/index";
import { useCallback, useMemo } from "react";
import { useHosts, useStore } from "#ui/use-store.ts";
import { Back } from "#ui/app/back.tsx";
import type { FunctionComponent } from "react";
import type { HostKey } from "@conductor/core/saved";
import { NewSession } from "#ui/session/new-session.tsx";
import { Route } from "@conductor/core/route";
import type { Send } from "#ui/link/host.ts";
import { nameOf } from "@conductor/core/store";

const NewSessionPane: FunctionComponent<{ readonly host: HostKey }> = ({ host }) => {
  const store = useStore();
  const paired = useHosts();
  const hosts = useMemo(
    () =>
      paired.map((found) => ({
        host: found.fold,
        key: found.key,
        name: nameOf(found),
        send: (body: Command): ReturnType<Send> => store.send(found.key, body),
      })),
    [paired, store],
  );
  const started = useCallback(
    (on: string, session: SessionId): void => {
      store.show(Route.Session({ host: on, session }));
    },
    [store],
  );
  return (
    <main className="pane" data-host={host}>
      <Back />
      <NewSession hosts={hosts} initial={host} key={host} started={started} />
    </main>
  );
};

export { NewSessionPane };
