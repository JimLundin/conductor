import type { Auth, ProviderInfo, SessionId } from "@conductor/core/schemas/index";
import { ListStatus, listStatus } from "@conductor/core/fold/offers";
import { Route, fragmentOf } from "@conductor/core/route";
import type { Host } from "@conductor/core/fold/host";
import type { HostKey } from "@conductor/core/saved";
import type { LinkState } from "@conductor/core/link";
import type { PairedHost } from "@conductor/core/store";
import type { Session } from "@conductor/core/fold/session";
import type { TicketError } from "@conductor/core/ticket";
import { agentLabel } from "@conductor/core/session/start";
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

const credentialLabel = (auth: Auth, provider: ProviderInfo | undefined): string => {
  if (auth === "SignedIn") {
    return "Signed in";
  }
  if (auth === "Unmanaged") {
    return "CLI-managed credentials";
  }
  return (
    provider?.credentials.find((form) => form.kind === auth.Stored.kind)?.label ?? auth.Stored.kind
  );
};

interface AccountSetup {
  readonly auth: string;
  readonly key: string;
  readonly label: string;
}

const accountSetup = (host: Host): ReadonlyArray<AccountSetup> =>
  host.instances.map(([instance, { availability }]) => {
    const provider = host.catalog?.[instance.provider];
    return {
      auth:
        "Available" in availability
          ? credentialLabel(availability.Available.auth, provider)
          : availability.Unavailable.reason,
      key: JSON.stringify([instance.provider, instance.account]),
      label: agentLabel(provider?.label ?? instance.provider, instance),
    };
  });

const pairingNotice = (notice: TicketError | undefined): string | undefined =>
  notice === undefined ? undefined : describeTicketError(notice);
const hostName = (key: HostKey, host: PairedHost | undefined): string =>
  host === undefined ? key : nameOf(host);

const workHref = (project = "", issue = ""): string => fragmentOf(Route.Work({ issue, project }));
const sessionsHref = fragmentOf(Route.Sessions());
const pairingHref = fragmentOf(Route.Pairing());
const newSessionHref = (host: HostKey): string => fragmentOf(Route.NewSession({ host }));

export {
  workHref,
  accountSetup,
  hostName,
  linkStates,
  newSessionHref,
  pairingHref,
  pairingNotice,
  sessionRow,
  sessionsHref,
};
