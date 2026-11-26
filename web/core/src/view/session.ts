import type { Session } from "@conductor/core/fold/session";
import { SessionStatus } from "@conductor/core/fold/session";
import { agentLabel } from "#core/session/start";
import { closeText } from "@conductor/core/session/words";
import { controls } from "@conductor/core/fold/offers";

const emptyText = {
  Closed: "The agent closed before any message.",
  Ready: "Send a message to start.",
  Starting: "Starting the agent…",
} as const;

interface Status {
  readonly name: string;
  readonly text: string;
}
interface Meta {
  readonly name: string;
  readonly text: string;
}
interface Header {
  readonly meta: ReadonlyArray<Meta>;
  readonly status: Status;
}
interface Composer {
  readonly disabled: boolean;
  readonly placeholder: string;
  readonly sendDisabled: boolean;
}

const status: (value: SessionStatus) => Status = SessionStatus.$match({
  Closed: ({ reason }) => ({ name: "closed", text: `Closed: it ${closeText(reason)}` }),
  Idle: () => ({ name: "idle", text: "Idle" }),
  Running: () => ({ name: "running", text: "Running" }),
  Starting: () => ({ name: "starting", text: "Starting" }),
});

const header = (session: Session, host: string): Header => ({
  meta: (
    [
      { name: "host", text: host },
      { name: "provider", text: agentLabel(session.instance.provider, session.instance) },
      { name: "model", text: session.model },
      { name: "effort", text: session.effort },
    ] as const
  ).flatMap((entry) =>
    typeof entry.text === "string" ? [{ name: entry.name, text: entry.text }] : [],
  ),
  status: status(session.status),
});

const composer = (session: Session, pending: boolean): Composer => {
  const offered = controls(session);
  return {
    disabled: offered.composer === "Closed",
    placeholder: offered.composer === "Busy" ? "The agent is working…" : "Message the agent",
    sendDisabled: offered.composer !== "Open" || pending,
  };
};

export { composer, emptyText, header };
