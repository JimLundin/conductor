import { emptyTranscript, isWorking } from "@conductor/core/fold/offers";
import { Composer } from "#ui/session/composer.tsx";
import type { FunctionComponent } from "react";
import { Header } from "#ui/session/header.tsx";
import { Items } from "#ui/session/transcript.tsx";
import { Requests } from "#ui/session/requests.tsx";
import type { Send } from "#ui/link/host.ts";
import type { Session } from "@conductor/core/fold/session";
import type { SessionId } from "@conductor/core/schemas/index";
import { emptyText } from "@conductor/core/view/session";

interface Props {
  readonly hostName: string;
  readonly id: SessionId;
  readonly send: Send;
  readonly session: Session;
}

const working = "Working…";

const SessionView: FunctionComponent<Props> = ({ hostName, id, send, session }) => {
  const empty = emptyTranscript(session);
  return (
    <section className="session">
      <Header hostName={hostName} session={session} />
      <div className="transcript">
        {typeof empty === "string" ? (
          <p className="empty">{emptyText[empty]}</p>
        ) : (
          <Items items={session.items} subagents={session.subagents} />
        )}
        {isWorking(session) ? <p className="working">{working}</p> : undefined}
      </div>
      <footer className="dock">
        <Requests id={id} send={send} session={session} />
        <Composer id={id} send={send} session={session} />
      </footer>
    </section>
  );
};

export { SessionView };
