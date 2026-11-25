// A host's welcome and feed folded into what the views render: the only fold of its events.
import { isDelta } from "../link/feed.ts";
import type { Update } from "../link/host.ts";
import type {
  Catalog,
  Event,
  Feed,
  SessionDirectory,
  Instances,
  Project,
  SessionId,
  Welcome,
} from "../schemas/index.ts";
import { applyKind, createdSession, type Session } from "./session.ts";

interface Host {
  // Null until the first welcome.
  readonly name: string | null;
  readonly catalog: Catalog | null;
  readonly instances: Instances;
  // The home's projects, where a new session may start.
  readonly projects: readonly Project[];
  readonly sessions: ReadonlyMap<SessionId, Session>;
}

type Sessions = Host["sessions"];

const without = (sessions: Sessions, id: SessionId): Sessions =>
  new Map([...sessions].filter(([known]) => known !== id));

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
  name: null,
  catalog: null,
  instances: [],
  projects: [],
  sessions: new Map(),
});

// A welcome replaces what the host advertises and keeps its sessions: a reconnect catches up
// after them.
const applyWelcome = (host: Host, welcome: Welcome): Host => ({
  ...host,
  name: welcome.host,
  catalog: welcome.catalog,
  instances: welcome.instances,
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
const applyUpdate = (host: Host, update: Update): Host => {
  switch (update._tag) {
    case "Reset":
      return resetHistory(host);
    case "Welcome":
      return applyWelcome(host, update.welcome);
    case "Feed":
      return applyFeed(host, update.frame);
    case "Status":
      return host;
  }
};

// The sessions, the most recently active first.
const recent = (host: Host): readonly SessionId[] =>
  [...host.sessions].toSorted(([, a], [, b]) => b.activity - a.activity).map(([id]) => id);

// Each directory a session ran in, once, the most recent first: the suggestions for a new
// session.
const directories = (host: Host): readonly SessionDirectory[] => [
  ...new Set(
    recent(host).flatMap((id) => {
      const session = host.sessions.get(id);
      return session === undefined ? [] : [session.directory];
    }),
  ),
];

export { applyUpdate, directories, emptyHost, type Host, recent };
