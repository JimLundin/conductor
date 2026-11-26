import type { Command, SessionId } from "@conductor/core/schemas/index";
import type { FunctionComponent, SubmitEvent, SubmitEventHandler } from "react";
import { blank, eligible, startOf } from "@conductor/core/session/start";
import { selection, startedSession } from "@conductor/core/view/start";
import { useCallback, useState } from "react";
import type { Draft } from "@conductor/core/session/start";
import { InstanceFields } from "#ui/session/instance-fields.tsx";
import { Notice } from "#ui/session/notice.tsx";
import type { PairedHost } from "@conductor/core/store";
import type { Send } from "#ui/link/host.ts";
import { WorkFields } from "#ui/session/work-fields.tsx";
import { useSending } from "#ui/session/sending.ts";

interface Props {
  readonly hosts: ReadonlyArray<PairedHost>;
  readonly directoryHost?: string | undefined;
  readonly project?: string | undefined;
  readonly issue?: string | undefined;
  readonly send: (host: string, command: Command) => ReturnType<Send>;
  readonly started: (host: string, session: SessionId) => void;
}

const title = "New session";

const NewSession: FunctionComponent<Props> = ({
  hosts,
  directoryHost,
  project = "",
  issue = "",
  send,
  started,
}) => {
  const [draft, setDraft] = useState<Draft>({ ...blank, issue, project });
  const sending = useSending();
  const options = eligible(hosts, draft, directoryHost);
  const chosen = selection(options, draft.agent);
  const command = startOf(chosen, draft);
  const change = useCallback((next: Partial<Draft>): void => {
    setDraft((current) => ({ ...current, ...next }));
  }, []);
  const submit = useCallback<SubmitEventHandler<HTMLFormElement>>(
    (event: Readonly<Pick<SubmitEvent<HTMLFormElement>, "preventDefault">>) => {
      event.preventDefault();
      if (chosen !== undefined && command !== undefined && !sending.pending) {
        const { key } = chosen.host;
        sending.run(
          (body) => send(key, body),
          command,
          (result) => {
            const session = startedSession(result);
            if (session !== undefined) {
              started(key, session);
            }
          },
        );
      }
    },
    [chosen, command, send, sending, started],
  );
  return (
    <form className="start-session" onSubmit={submit}>
      <h1>{title}</h1>
      <WorkFields change={change} chosen={chosen} draft={draft} hosts={hosts} />
      <InstanceFields change={change} chosen={chosen} draft={draft} options={options} />
      <Notice tone="notice" text={sending.notice} />
      <button className="primary" disabled={command === undefined || sending.pending} type="submit">
        {chosen === undefined ? "Start session" : `Start with ${chosen.label}`}
      </button>
    </form>
  );
};

export { NewSession };
