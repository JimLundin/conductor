import type {
  Catalog,
  Event,
  Feed,
  Instances,
  Project,
  SessionDirectory,
  SessionId,
  Welcome,
} from "#core/schemas/index";
import { Update, isDelta } from "#core/link";
import { applyKind, createdSession } from "#core/fold/session";
import type { Session } from "#core/fold/session";
// A host's welcome and feed folded into what the views render: the only fold of its events.

interface Host {
  // Absent until the first welcome.
  readonly name: string | undefined;
  readonly catalog: Catalog | undefined;
  readonly instances: Instances;
  // The home's projects, where a new session may start.
  readonly projects: ReadonlyArray<Project>;
  readonly sessions: Readonly<ReadonlyMap<SessionId, Session>>;
}

type Sessions = Host["sessions"];
type SessionEntry = readonly [SessionId, Session];

const without = (sessions: Sessions, id: SessionId): Sessions =>
  new Map([...sessions].filter(([known]: SessionEntry) => known !== id));

const applied = (sessions: Sessions, event: Event): Sessions => {
  const session = sessions.get(event.session);
  if (session === undefined) {
    return sessions;
  }
  const next = applyKind(session, event.kind);
  return new Map(sessions).set(
    event.session,
    isDelta(event) ? next : { ...next, activity: event.seq },
  );
};

// An event for a session never seen created is dropped, and a removed session is gone.
const applyEvent = (sessions: Sessions, event: Event): Sessions => {
  const { kind } = event;
  if (typeof kind === "object" && "SessionCreated" in kind) {
    return new Map(sessions).set(event.session, createdSession(kind.SessionCreated, event.seq));
  }
  if (typeof kind === "object" && "SessionRemoved" in kind) {
    return without(sessions, event.session);
  }
  return applied(sessions, event);
};

const emptyHost = (): Host => ({
  catalog: undefined,
  instances: [],
  name: undefined,
  projects: [],
  sessions: new Map(),
});

// A welcome replaces what the host advertises and keeps its sessions: a reconnect catches up
// After them.
const applyWelcome = (host: Host, welcome: Welcome): Host => ({
  ...host,
  catalog: welcome.catalog,
  instances: welcome.instances,
  name: welcome.host,
  projects: welcome.home?.projects ?? [],
});

// The link resets a host's history on a fresh cursor or a new epoch, before the replay.
const resetHistory = (host: Host): Host => ({ ...host, sessions: new Map() });

// Sign-ins and the home's issues and flow are not part of the thin app.
const applyFeed = (host: Host, feed: Feed): Host => {
  if ("Event" in feed) {
    return { ...host, sessions: applyEvent(host.sessions, feed.Event) };
  }
  if ("Instances" in feed) {
    return { ...host, instances: feed.Instances };
  }
  if ("Home" in feed) {
    return { ...host, projects: feed.Home.projects };
  }
  return host;
};

// A link status is not part of the host's fold.
const applyUpdate = (host: Host, update: Update): Host =>
  Update.$match(update, {
    Feed: ({ frame }) => applyFeed(host, frame),
    Reset: () => resetHistory(host),
    Status: () => host,
    Welcome: ({ welcome }) => applyWelcome(host, welcome),
  });

// The sessions, the most recently active first.
const recent = (host: Host): ReadonlyArray<SessionId> =>
  [...host.sessions]
    .toSorted(
      ([, first]: SessionEntry, [, second]: SessionEntry) => second.activity - first.activity,
    )
    .map(([id]: SessionEntry) => id);

// Each directory a session ran in, once, the most recent first: the suggestions for a new
// Session.
const directories = (host: Host): ReadonlyArray<SessionDirectory> => [
  ...new Set(
    recent(host).flatMap((id) => {
      const session = host.sessions.get(id);
      return session === undefined ? [] : [session.directory];
    }),
  ),
];

export { applyUpdate, directories, emptyHost, type Host, recent };
