import { HostAction, HostOperation, stepsOf } from "#core/pairing";
import type { HostKey, Saved } from "#core/saved";
import { Route, route } from "#core/route";
import type { SessionId, Ticket } from "#core/schemas/index";
import { applyUpdate, emptyHost, recent } from "#core/fold/host";
import type { Host } from "#core/fold/host";
import type { Session } from "#core/fold/session";
import type { Status } from "#core/link";
import type { Steps } from "#core/pairing";
import type { TicketError } from "#core/ticket";
import { Update } from "#core/link";
import { hostKey } from "#core/saved";

interface PairedHost {
  readonly key: HostKey;
  readonly ticket: Ticket;
  readonly saved: string | undefined;
  readonly status: Status;
  readonly fold: Host;
  readonly operation: HostOperation;
}

interface View {
  readonly hosts: Readonly<ReadonlyMap<HostKey, PairedHost>>;
  readonly route: Route;
  readonly notice: TicketError | undefined;
  readonly restoration: "Ready" | "Failed";
  readonly forgotten: Readonly<ReadonlySet<HostKey>>;
  readonly operations: number;
}

interface Requested {
  readonly view: View;
  readonly host: PairedHost;
  readonly steps: Steps;
}

interface Updated {
  readonly view: View;
  readonly saved: Saved | undefined;
}

const hostLength = 8;
const firstCharacter = 0;
const noHosts = 0;
const oneOperation = 1;

const initialView = (): View => ({
  forgotten: new Set(),
  hosts: new Map(),
  notice: undefined,
  operations: 0,
  restoration: "Ready",
  route: Route.Pairing(),
});

const nameOf = (host: PairedHost): string =>
  host.fold.name ?? host.saved ?? `host ${host.key.slice(firstCharacter, hostLength)}`;

const listed = (view: View): ReadonlyArray<PairedHost> =>
  [...view.hosts.values()].toSorted(
    (first, second) =>
      nameOf(first).localeCompare(nameOf(second)) || first.key.localeCompare(second.key),
  );

const sessionsOf = (view: View, host: HostKey): ReadonlyArray<SessionId> => {
  const paired = view.hosts.get(host);
  return paired === undefined ? [] : recent(paired.fold);
};

const sessionOf = (view: View, host: HostKey, session: SessionId): Session | undefined =>
  view.hosts.get(host)?.fold.sessions.get(session);

const withUpdate = (host: PairedHost, update: Update): PairedHost => {
  if (Update.$is("Status")(update)) {
    return { ...host, status: { error: update.error, state: update.state } };
  }
  return {
    ...host,
    fold: applyUpdate(host.fold, update),
    saved: Update.$is("Welcome")(update) ? update.welcome.host : host.saved,
  };
};

const shown = (fragment: string, hosts: View["hosts"]): Route => {
  const screen = route(fragment, hosts.size > noHosts);
  if (
    (Route.$is("Session")(screen) || Route.$is("NewSession")(screen)) &&
    !hosts.has(screen.host)
  ) {
    return Route.Sessions();
  }
  return screen;
};

const sameTicket = (first: Ticket, second: Ticket): boolean =>
  JSON.stringify(first) === JSON.stringify(second);

const requestHost = (
  view: View,
  host: Omit<PairedHost, "operation">,
  action: HostAction,
): Requested => {
  const steps = stepsOf(action);
  const requested: PairedHost = {
    ...host,
    fold: HostAction.$is("Forget")(action) ? emptyHost() : host.fold,
    operation: HostOperation.Pending({ action, id: view.operations }),
    status: steps.stop ? { error: undefined, state: "Disconnected" } : host.status,
  };
  const forgotten = new Set(view.forgotten);
  forgotten.delete(host.key);
  return {
    host: requested,
    steps,
    view: {
      ...view,
      forgotten,
      hosts: new Map(view.hosts).set(host.key, requested),
      operations: view.operations + oneOperation,
    },
  };
};

const adopted = (view: View, ticket: Ticket, action: HostAction): boolean => {
  const key = hostKey(ticket);
  const known = view.hosts.get(key);
  if (HostAction.$is("Restore")(action)) {
    return known === undefined && !view.forgotten.has(key);
  }
  return (
    known === undefined ||
    !HostOperation.$is("Complete")(known.operation) ||
    !sameTicket(known.ticket, ticket)
  );
};

const adoptHost = (view: View, saved: Saved, action: HostAction): Requested | undefined => {
  if (!adopted(view, saved.ticket, action)) {
    return undefined;
  }
  const { name, ticket } = saved;
  const key = hostKey(ticket);
  const known = view.hosts.get(key);
  return requestHost(
    view,
    {
      fold: known?.fold ?? emptyHost(),
      key,
      saved: known?.saved ?? name,
      status: { error: undefined, state: "Disconnected" },
      ticket,
    },
    action,
  );
};

const applyPairedUpdate = (view: View, key: HostKey, update: Update): Updated => {
  const host = view.hosts.get(key);
  if (host === undefined) {
    return { saved: undefined, view };
  }
  const next = withUpdate(host, update);
  return {
    saved: next.saved === host.saved ? undefined : { name: next.saved, ticket: next.ticket },
    view: { ...view, hosts: new Map(view.hosts).set(key, next) },
  };
};

const settledHost = (view: View, key: HostKey, operation: HostOperation): View => {
  const host = view.hosts.get(key);
  if (host?.operation.id !== operation.id) {
    return view;
  }
  const hosts = new Map(view.hosts);
  if (HostOperation.$is("Complete")(operation) && HostAction.$is("Forget")(operation.action)) {
    hosts.delete(key);
    return { ...view, forgotten: new Set(view.forgotten).add(key), hosts };
  }
  hosts.set(key, { ...host, operation });
  return { ...view, hosts };
};

const rememberAction = (host: PairedHost): HostAction | undefined =>
  HostOperation.$is("Complete")(host.operation) || HostAction.$is("Remember")(host.operation.action)
    ? HostAction.Remember()
    : undefined;

export {
  rememberAction,
  requestHost,
  settledHost,
  adoptHost,
  applyPairedUpdate,
  initialView,
  listed,
  nameOf,
  sessionOf,
  sessionsOf,
  shown,
};
export type { PairedHost, Requested, View };
