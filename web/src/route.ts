import { Data } from "effect";
import type { SessionId } from "./proto/index.ts";
import type { HostKey } from "./saved.ts";

// The screen a URL fragment names. Pages serves one path, so every screen is a fragment.
export type Route = Data.TaggedEnum<{
	readonly Pairing: Record<never, never>;
	// Every paired host with its sessions: the sidebar, and on a phone the stack's first screen.
	readonly Sessions: Record<never, never>;
	readonly NewSession: { readonly host: HostKey };
	readonly Session: { readonly host: HostKey; readonly session: SessionId };
}>;

export const Route = Data.taggedEnum<Route>();

const join = "#join=";

// Whether the fragment is a join link's, which pairs its ticket instead of naming a screen.
export const isJoin = (fragment: string): boolean => fragment.startsWith(join);

const screen = (parts: readonly string[]): Route => {
	const [first, host, kind, session] = parts;
	if (first === "pair" && parts.length === 1) {
		return Route.Pairing();
	}
	if (first === "host" && host !== undefined && host !== "") {
		if (kind === "new" && parts.length === 3) {
			return Route.NewSession({ host });
		}
		if (kind === "session" && session !== undefined && parts.length === 4) {
			return Route.Session({ host, session });
		}
	}
	return Route.Sessions();
};

// With no host paired, every fragment shows pairing; an unknown one shows the sessions.
export const route = (fragment: string, paired: boolean): Route => {
	if (!paired) {
		return Route.Pairing();
	}
	return screen(fragment.replace(/^#\/?/v, "").split("/"));
};

export const fragmentOf: (route: Route) => string = Route.$match({
	Pairing: () => "#/pair",
	Sessions: () => "#/",
	NewSession: ({ host }) => `#/host/${host}/new`,
	Session: ({ host, session }) => `#/host/${host}/session/${session}`,
});
