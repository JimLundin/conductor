import { Result } from "effect";
import { type ReactElement, useId, useState } from "react";
import { type ListStatus, listStatus } from "./fold/offers.ts";
import type { Session } from "./fold/session.ts";
import type { LinkState, Status } from "./link/host.ts";
import type { Command, SessionId } from "./schemas/index.ts";
import { fragmentOf, Route } from "./route.ts";
import { NewSession } from "./session/new-session.tsx";
import { SessionView } from "./session/session-view.tsx";
import type { HostKey } from "./saved.ts";
import { nameOf, type PairedHost, type Store } from "./store.ts";
import { describeTicketError } from "./ticket.ts";
import {
  useHost,
  useHosts,
  useRoute,
  useSession,
  useSessions,
  useStore,
  useView,
} from "./use-store.ts";

const linkStates = {
  Disconnected: "Offline",
  Connecting: "Connecting",
  Authenticating: "Authenticating",
  CatchingUp: "Catching up",
  Live: "Live",
} as const satisfies Readonly<Record<LinkState, string>>;

const describeListStatus: (status: ListStatus) => string = (status) => {
  if (status._tag === "Requests") {
    return status.count === 1 ? "1 request waiting" : `${String(status.count)} requests waiting`;
  }
  return status.status;
};

// A session's directory's last part names it in the list.
const sessionTitle = (session: Session): string =>
  session.directory.split(/[\/\\]/v).findLast((part) => part !== "") ?? session.directory;

const LinkStatus = ({ status }: { readonly status: Status }): ReactElement => (
  <span className="link-status" data-state={status.state} title={status.error}>
    {linkStates[status.state]}
  </span>
);

const SessionRow = ({
  host,
  id,
}: {
  readonly host: HostKey;
  readonly id: SessionId;
}): ReactElement | null => {
  const session = useSession(host, id);
  const route = useRoute();
  if (session === undefined) {
    return null;
  }
  const current = route._tag === "Session" && route.host === host && route.session === id;
  const status = listStatus(session);
  return (
    <li>
      <a
        aria-current={current ? "page" : undefined}
        className="session-row"
        href={fragmentOf(Route.Session({ host, session: id }))}
      >
        <span className="session-title">{sessionTitle(session)}</span>
        <span
          className="session-status"
          data-status={status._tag === "Requests" ? "Requests" : status.status}
        >
          {describeListStatus(status)}
        </span>
      </a>
    </li>
  );
};

const HostGroup = ({ host }: { readonly host: PairedHost }): ReactElement => {
  const sessions = useSessions(host.key);
  return (
    <section className="host-group" data-host={host.key}>
      <header className="host-header">
        <h2>{nameOf(host)}</h2>
        <LinkStatus status={host.status} />
        <a className="new-session" href={fragmentOf(Route.NewSession({ host: host.key }))}>
          New session
        </a>
      </header>
      {host.status.error === undefined ? null : <p className="link-error">{host.status.error}</p>}
      {sessions.length === 0 ? (
        <p className="empty">No sessions yet.</p>
      ) : (
        <ul className="sessions">
          {sessions.map((id) => (
            <SessionRow host={host.key} id={id} key={id} />
          ))}
        </ul>
      )}
    </section>
  );
};

const Sidebar = (): ReactElement => (
  <nav className="sidebar" aria-label="Sessions">
    <header className="band">
      <span className="band-title">Conductor</span>
      <a className="add-host" href={fragmentOf(Route.Pairing())}>
        Hosts
      </a>
    </header>
    {useHosts().map((host) => (
      <HostGroup host={host} key={host.key} />
    ))}
  </nav>
);

const pairing = {
  title: "Pair a host",
  hint: "Open a host's join link on this device, or paste the join link or ticket it prints.",
} as const;

const PairForm = (): ReactElement => {
  const store = useStore();
  const notice = useView().notice;
  const [error, setError] = useState<string | null>(null);
  const field = useId();
  const pair = (data: FormData): void => {
    const ticket = data.get("ticket");
    const paired = store.pair(typeof ticket === "string" ? ticket : "");
    setError(Result.isFailure(paired) ? describeTicketError(paired.failure) : null);
  };
  const shown = error ?? (notice === null ? null : describeTicketError(notice));
  return (
    <form action={pair} className="pair-form">
      <label htmlFor={field}>Join link or ticket</label>
      <textarea
        autoCapitalize="off"
        autoComplete="off"
        id={field}
        name="ticket"
        required={true}
        rows={3}
        spellCheck={false}
      />
      {shown === null ? null : (
        <p className="error" role="alert">
          {shown}
        </p>
      )}
      <button type="submit">Pair</button>
    </form>
  );
};

const PairedRow = ({ host }: { readonly host: PairedHost }): ReactElement => {
  const store = useStore();
  return (
    <li className="paired" data-host={host.key}>
      <span className="paired-name">{nameOf(host)}</span>
      <LinkStatus status={host.status} />
      <button className="forget" onClick={(): void => store.forget(host.key)} type="button">
        Forget
      </button>
    </li>
  );
};

const Pairing = (): ReactElement => {
  const hosts = useHosts();
  return (
    <main className="pairing">
      {hosts.length === 0 ? null : (
        <a className="back" href={fragmentOf(Route.Sessions())}>
          Sessions
        </a>
      )}
      <h1>{pairing.title}</h1>
      <p>{pairing.hint}</p>
      <PairForm />
      {hosts.length === 0 ? null : (
        <ul className="paired-hosts">
          {hosts.map((host) => (
            <PairedRow host={host} key={host.key} />
          ))}
        </ul>
      )}
    </main>
  );
};

const Back = (): ReactElement => (
  <a className="back" href={fragmentOf(Route.Sessions())}>
    Sessions
  </a>
);

const SessionPane = ({
  host,
  session,
}: {
  readonly host: HostKey;
  readonly session: SessionId;
}): ReactElement => {
  const paired = useHost(host);
  const found = useSession(host, session);
  const { send } = useStore();
  return (
    <main className="pane" data-host={host} data-session={session}>
      <Back />
      {found === undefined ? (
        <p className="empty">The host has no such session.</p>
      ) : (
        <SessionView
          hostName={paired === undefined ? host : nameOf(paired)}
          id={session}
          send={(body) => send(host, body)}
          session={found}
        />
      )}
    </main>
  );
};

const NewSessionPane = ({ host }: { readonly host: HostKey }): ReactElement => {
  const store = useStore();
  const hosts = useHosts().map((paired) => ({
    key: paired.key,
    name: nameOf(paired),
    host: paired.fold,
    send: (body: Command): ReturnType<Store["send"]> => store.send(paired.key, body),
  }));
  return (
    <main className="pane" data-host={host}>
      <Back />
      <NewSession
        hosts={hosts}
        initial={host}
        key={host}
        started={(on, session): void => store.show(Route.Session({ host: on, session }))}
      />
    </main>
  );
};

const Chosen = (): ReactElement => (
  <main className="pane pane-empty">
    <p>Choose a session, or start a new one on a host.</p>
  </main>
);

const pane = Route.$match({
  Pairing: () => <Pairing />,
  Sessions: () => <Chosen />,
  NewSession: ({ host }) => <NewSessionPane host={host} />,
  Session: ({ host, session }) => <SessionPane host={host} session={session} />,
});

// The sidebar beside the shown pane; on a phone they are a stack, showing the list on the
// sessions route and the pane on any other.
export const App = (): ReactElement => {
  const route = useRoute();
  if (route._tag === "Pairing") {
    return <Pairing />;
  }
  return (
    <div className="shell" data-top={route._tag === "Sessions" ? "list" : "pane"}>
      <Sidebar />
      {pane(route)}
    </div>
  );
};
