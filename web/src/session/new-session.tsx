import type { CommandResult, SessionId } from "@conductor/core/schemas/index";
import type { FunctionComponent, SubmitEvent, SubmitEventHandler } from "react";
import { start, startDisabled, startedSession } from "@conductor/core/view/start";
import { useCallback, useState } from "react";
import type { Draft } from "@conductor/core/session/start";
import type { Host } from "@conductor/core/fold/host";
import { HostField } from "#ui/session/host-field.tsx";
import { InstanceFields } from "#ui/session/instance-fields.tsx";
import { Notice } from "#ui/session/notice.tsx";
import type { Send } from "#ui/link/host.ts";
import { blank } from "@conductor/core/session/start";
import { useSending } from "#ui/session/sending.ts";

interface Paired {
  readonly host: Host;
  readonly key: string;
  readonly name: string;
  readonly send: Send;
}

interface Props {
  readonly hosts: ReadonlyArray<Paired>;
  readonly initial: string;
  readonly started: (host: string, session: SessionId) => void;
}

const title = "New session";
const startLabel = "Start";

const notifyStarted = (result: CommandResult, host: string, started: Props["started"]): void => {
  const session = startedSession(result);
  if (session !== undefined) {
    started(host, session);
  }
};

const NewSession: FunctionComponent<Props> = ({ hosts, initial, started }) => {
  const [draft, setDraft] = useState<Draft>({ ...blank, host: initial });
  const paired = hosts.find((host) => host.key === draft.host);
  const sending = useSending();
  const command = start(paired?.host, draft);
  const change = useCallback((next: Partial<Draft>): void => {
    setDraft((current) => ({ ...current, ...next }));
  }, []);
  const chooseHost = useCallback((host: string): void => {
    setDraft({ ...blank, host });
  }, []);
  const submit = useCallback<SubmitEventHandler<HTMLFormElement>>(
    (event: Readonly<Pick<SubmitEvent<HTMLFormElement>, "preventDefault">>) => {
      event.preventDefault();
      if (command !== undefined && paired !== undefined) {
        sending.run(paired.send, command, (result) => {
          notifyStarted(result, paired.key, started);
        });
      }
    },
    [command, paired, sending, started],
  );
  return (
    <form className="start-session" onSubmit={submit}>
      <h1>{title}</h1>
      <HostField change={chooseHost} hosts={hosts} value={draft.host} />
      <InstanceFields change={change} draft={draft} host={paired?.host} />
      <Notice tone="notice" text={sending.notice} />
      <button className="primary" disabled={startDisabled(command, sending.pending)} type="submit">
        {startLabel}
      </button>
    </form>
  );
};

export { NewSession };
