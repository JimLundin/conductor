import type { Command, SessionId } from "@conductor/core/schemas/index";
import { hiddenRequests, requests } from "@conductor/core/view/requests";
import type { FunctionComponent } from "react";
import { Notice } from "#ui/session/notice.tsx";
import { RequestCard } from "#ui/session/request-card.tsx";
import type { Send } from "#ui/link/host.ts";
import type { Session } from "@conductor/core/fold/session";
import { useCallback } from "react";
import { useSending } from "#ui/session/sending.ts";

interface Props {
  readonly id: SessionId;
  readonly send: Send;
  readonly session: Session;
}

const Requests: FunctionComponent<Props> = ({ id, send, session }) => {
  const sending = useSending();
  const open = requests(session);
  const run = useCallback(
    (body: Command): void => {
      sending.run(send, body);
    },
    [send, sending],
  );
  return hiddenRequests(open.length, sending.notice) ? undefined : (
    <div className="requests">
      <Notice tone="notice" text={sending.notice} />
      {open.map((request) => (
        <RequestCard
          disabled={sending.pending}
          id={id}
          key={request.key}
          request={request}
          run={run}
        />
      ))}
    </div>
  );
};

export { Requests };
