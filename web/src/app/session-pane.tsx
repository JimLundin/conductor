import type { Command, SessionId } from "@conductor/core/schemas/index";
import { useHost, useSession, useStore } from "#ui/use-store.ts";
import { Back } from "#ui/app/back.tsx";
import type { FunctionComponent } from "react";
import type { HostKey } from "@conductor/core/saved";
import { SessionView } from "#ui/session/session-view.tsx";
import { hostName } from "@conductor/core/view/app";
import { useCallback } from "react";

interface Props {
  readonly host: HostKey;
  readonly session: SessionId;
}
const missing = "The host has no such session.";

const SessionPane: FunctionComponent<Props> = ({ host, session }) => {
  const paired = useHost(host);
  const found = useSession(host, session);
  const store = useStore();
  const send = useCallback((body: Command) => store.send(host, body), [host, store]);
  return (
    <main className="pane" data-host={host} data-session={session}>
      <Back />
      {found === undefined ? (
        <p className="empty">{missing}</p>
      ) : (
        <SessionView hostName={hostName(host, paired)} id={session} send={send} session={found} />
      )}
    </main>
  );
};

export { SessionPane };
