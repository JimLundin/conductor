import type { IssueId, ProjectId, SessionId } from "#core/schemas/index";
import { Data } from "effect";
import type { HostKey } from "#core/saved";
import type { Unit } from "#core/data";
import { joinMarker } from "#core/ticket";

type Route = Data.TaggedEnum<{
  readonly Pairing: Unit;
  readonly Work: { readonly project: ProjectId; readonly issue: IssueId };
  readonly Sessions: Unit;
  readonly NewSession: { readonly host: HostKey };
  readonly Session: { readonly host: HostKey; readonly session: SessionId };
}>;

const Route = Data.taggedEnum<Route>();

const isJoin = (fragment: string): boolean => fragment.startsWith(joinMarker);

const pairingParts = 1;
const newSessionParts = 3;
const sessionParts = 4;
const projectParts = 2;

const workScreen = (parts: ReadonlyArray<string>): Route => {
  const [first, host, kind, session] = parts;
  if (first === "new" && parts.length === pairingParts) {
    return Route.Work({ issue: "", project: "" });
  }
  if (first === "project" && host !== undefined && host !== "") {
    if (parts.length === projectParts) {
      return Route.Work({ issue: "", project: host });
    }
    if (
      kind === "issue" &&
      session !== undefined &&
      session !== "" &&
      parts.length === sessionParts
    ) {
      return Route.Work({ issue: session, project: host });
    }
  }
  return Route.Sessions();
};

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
  return workScreen(parts);
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
  Work: ({ project, issue }) => {
    if (project === "") {
      return "#/new";
    }
    return issue === "" ? `#/project/${project}` : `#/project/${project}/issue/${issue}`;
  },
});

export { fragmentOf, isJoin, Route, route };
