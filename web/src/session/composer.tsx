import type { ChangeEventHandler, FunctionComponent, SubmitEvent, SubmitEventHandler } from "react";
import type { Command, SessionId } from "@conductor/core/schemas/index";
import { message, sessionOffers } from "@conductor/core/session/commands";
import { useCallback, useState } from "react";
import { Notice } from "#ui/session/notice.tsx";
import { Offers } from "#ui/session/offers.tsx";
import type { Send } from "#ui/link/host.ts";
import type { Session } from "@conductor/core/fold/session";
import type { ValueChange } from "#ui/event.ts";
import { composer } from "@conductor/core/view/session";
import { useSending } from "#ui/session/sending.ts";

interface Props {
  readonly id: SessionId;
  readonly send: Send;
  readonly session: Session;
}

const sendLabel = "Send";

const Composer: FunctionComponent<Props> = ({ id, send, session }) => {
  const [text, setText] = useState("");
  const sending = useSending();
  const shown = composer(session, sending.pending);
  const submit = useCallback<SubmitEventHandler<HTMLFormElement>>(
    (event: Readonly<Pick<SubmitEvent<HTMLFormElement>, "preventDefault">>) => {
      event.preventDefault();
      if (text.trim() !== "") {
        sending.run(send, message(id, text), () => {
          setText("");
        });
      }
    },
    [id, send, sending, text],
  );
  const change = useCallback<ChangeEventHandler<HTMLTextAreaElement>>((event: ValueChange) => {
    setText(event.target.value);
  }, []);
  const run = useCallback(
    (body: Command): void => {
      sending.run(send, body);
    },
    [send, sending],
  );
  const offers = sessionOffers(id, session);
  return (
    <form className="composer" onSubmit={submit}>
      <Notice tone="notice" text={sending.notice} />
      <textarea
        aria-label="Message"
        disabled={shown.disabled}
        onChange={change}
        placeholder={shown.placeholder}
        value={text}
      />
      <div className="offers">
        <Offers disabled={sending.pending} offers={offers} run={run} />
        <button className="primary" disabled={shown.sendDisabled} type="submit">
          {sendLabel}
        </button>
      </div>
    </form>
  );
};

export { Composer };
