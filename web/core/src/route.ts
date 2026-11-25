import { Data } from "effect";
import type { HostKey } from "#core/saved";
import type { SessionId } from "#core/schemas/index";
import type { Unit } from "#core/data";
import { joinMarker } from "#core/ticket";

type Route = Data.TaggedEnum<{
  readonly Pairing: Unit;
  readonly Sessions: Unit;
  readonly NewSession: { readonly host: HostKey };
  readonly Session: { readonly host: HostKey; readonly session: SessionId };
}>;

const Route = Data.taggedEnum<Route>();

const isJoin = (fragment: string): boolean => fragment.startsWith(joinMarker);

const pairingParts = 1;
const newSessionParts = 3;
const sessionParts = 4;

const screen = (parts: ReadonlyArray<string>): Route => {
  const [first, host, kind, session] = parts;
  if (first === "pair" && parts.length === pairingParts) {
    return Route.Pairing();
  }
  if (first === "host" && host !== undefined && host !== "") {
    if (kind === "new" && parts.length === newSessionParts) {
      return Route.NewSession({ host });
    }
    if (kind === "session" && session !== undefined && parts.length === sessionParts) {
      return Route.Session({ host, session });
    }
  }
  return Route.Sessions();
};

const route = (fragment: string, paired: boolean): Route => {
  if (!paired) {
    return Route.Pairing();
  }
  return screen(fragment.replace(/^#\/?/v, "").split("/"));
};

const fragmentOf: (route: Route) => string = Route.$match({
  NewSession: ({ host }) => `#/host/${host}/new`,
  Pairing: () => "#/pair",
  Session: ({ host, session }) => `#/host/${host}/session/${session}`,
  Sessions: () => "#/",
});

export { fragmentOf, isJoin, Route, route };
