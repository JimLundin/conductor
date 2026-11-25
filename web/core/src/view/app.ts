import { ListStatus, listStatus } from "@conductor/core/fold/offers";
import { Route, fragmentOf } from "@conductor/core/route";
import type { HostKey } from "@conductor/core/saved";
import type { LinkState } from "@conductor/core/link";
import type { PairedHost } from "@conductor/core/store";
import { Result } from "effect";
import type { Session } from "@conductor/core/fold/session";
import type { SessionId } from "@conductor/core/schemas/index";
import type { TicketError } from "@conductor/core/ticket";
import { describeTicketError } from "@conductor/core/ticket";
import { nameOf } from "@conductor/core/store";

const linkStates = {
  Authenticating: "Authenticating",
  CatchingUp: "Catching up",
  Connecting: "Connecting",
  Disconnected: "Offline",
  Live: "Live",
} as const satisfies Readonly<Record<LinkState, string>>;
const singular = 1;

interface Address {
  readonly host: HostKey;
  readonly id: SessionId;
}

interface Row {
  readonly current: "page" | undefined;
  readonly href: string;
  readonly status: string;
  readonly text: string;
  readonly title: string;
}

const describeRequests = (count: number): string =>
  count === singular ? "1 request waiting" : `${String(count)} requests waiting`;
const sessionRow = (session: Session, route: Route, address: Address): Row => {
  const status = listStatus(session);
  const current =
    Route.$is("Session")(route) && route.host === address.host && route.session === address.id;
  const described = ListStatus.$is("Requests")(status)
    ? describeRequests(status.count)
    : status.status;
  return {
    current: current ? ("page" as const) : undefined,
    href: fragmentOf(Route.Session({ host: address.host, session: address.id })),
    status: ListStatus.$is("Requests")(status) ? "Requests" : status.status,
    text: described,
    title: session.directory.split(/[\/\\]/v).findLast((part) => part !== "") ?? session.directory,
  };
};

const pairingError = (paired: Result.Result<HostKey, TicketError>): string | undefined =>
  Result.isFailure(paired) ? describeTicketError(paired.failure) : undefined;
const pairingNotice = (
  error: string | undefined,
  notice: TicketError | undefined,
): string | undefined => error ?? (notice === undefined ? undefined : describeTicketError(notice));
const hostName = (key: HostKey, host: PairedHost | undefined): string =>
  host === undefined ? key : nameOf(host);

const sessionsHref = fragmentOf(Route.Sessions());
const pairingHref = fragmentOf(Route.Pairing());
const newSessionHref = (host: HostKey): string => fragmentOf(Route.NewSession({ host }));

export {
  hostName,
  linkStates,
  newSessionHref,
  pairingError,
  pairingHref,
  pairingNotice,
  sessionRow,
  sessionsHref,
};
