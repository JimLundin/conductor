// The app's one store: every paired host with its link status and fold, and the screen shown.
// Each host's link runs in its own fiber, whose scope closing is how forgetting works.
import { Effect, FiberMap, Result, type Scope, Stream } from "effect";
import {
	applyFeed,
	applyWelcome,
	emptyHost,
	type Host,
	recent,
	resetHistory,
} from "./fold/host.ts";
import type { Session } from "./fold/session.ts";
import { link } from "./link.ts";
import { SendError } from "./link/command.ts";
import { type HostLink, type Status, Update } from "./link/host.ts";
import type { Pipe } from "./pipe.ts";
import type {
	ClientName,
	Command,
	CommandResult,
	SessionId,
	Ticket,
} from "./schemas/index.ts";
import { fragmentOf, isJoin, Route, route } from "./route.ts";
import { type HostKey, hostKey, restore, save, unsave } from "./saved.ts";
import { parseJoin, type TicketError } from "./ticket.ts";

interface PairedHost {
	readonly key: HostKey;
	readonly ticket: Ticket;
	// The name from its last welcome, kept across launches.
	readonly saved: string | null;
	readonly status: Status;
	readonly fold: Host;
}

interface View {
	// In the order they were paired; `listed` gives the order the app shows.
	readonly hosts: ReadonlyMap<HostKey, PairedHost>;
	readonly route: Route;
	// Why the last join link paired nothing, until the next pairing.
	readonly notice: TicketError | null;
}

// What the store needs from the page, passed in so tests can play it.
interface Boundary {
	readonly pipe: Pipe;
	readonly client: ClientName;
	readonly storage: Storage;
	readonly fragment: () => string;
	// Shows `fragment` as a new history entry, so Back returns down the phone's stack.
	readonly go: (fragment: string) => void;
	// Shows `fragment` in place of the current entry, so a join ticket stays out of history.
	readonly replace: (fragment: string) => void;
}

interface Store {
	readonly view: () => View;
	readonly subscribe: (changed: () => void) => () => void;
	// Pairs the host a join link or bare ticket names. Pairing a paired host again replaces
	// its ticket and keeps its record.
	readonly pair: (text: string) => Result.Result<HostKey, TicketError>;
	readonly forget: (host: HostKey) => void;
	// Runs one command on the host's link; see HostLink.send.
	readonly send: (
		host: HostKey,
		body: Command,
	) => Effect.Effect<CommandResult, SendError>;
	readonly show: (route: Route) => void;
	// Reads the fragment again, after the page's changed.
	readonly follow: () => void;
	// Drops each link's connection and connects again, as after a network change.
	readonly reconnect: () => void;
}

const disconnected = {
	state: "Disconnected",
	error: undefined,
} as const satisfies Status;

const hostLength = 8;

// What the app calls a host: the name it gave, else a short form of its id.
const nameOf = (host: PairedHost): string =>
	host.fold.name ?? host.saved ?? `host ${host.key.slice(0, hostLength)}`;

// The hosts as the app lists them: by name, then by id.
const listed = (view: View): readonly PairedHost[] =>
	[...view.hosts.values()].toSorted(
		(a, b) => nameOf(a).localeCompare(nameOf(b)) || a.key.localeCompare(b.key),
	);

// A host's sessions, the most recently active first.
const sessionsOf = (view: View, host: HostKey): readonly SessionId[] => {
	const paired = view.hosts.get(host);
	return paired === undefined ? [] : recent(paired.fold);
};

const sessionOf = (
	view: View,
	host: HostKey,
	session: SessionId,
): Session | undefined => view.hosts.get(host)?.fold.sessions.get(session);

const applyUpdate = (host: PairedHost, update: Update): PairedHost => {
	switch (update._tag) {
		case "Status":
			return { ...host, status: { state: update.state, error: update.error } };
		case "Reset":
			return { ...host, fold: resetHistory(host.fold) };
		case "Welcome":
			return {
				...host,
				saved: update.welcome.host,
				fold: applyWelcome(host.fold, update.welcome),
			};
		case "Feed":
			return { ...host, fold: applyFeed(host.fold, update.frame) };
	}
};

// A screen of a host no longer paired shows the sessions instead.
const shown = (fragment: string, hosts: View["hosts"]): Route => {
	const screen = route(fragment, hosts.size > 0);
	if (
		(screen._tag === "Session" || screen._tag === "NewSession") &&
		!hosts.has(screen.host)
	) {
		return Route.Sessions();
	}
	return screen;
};

const sameTicket = (a: Ticket, b: Ticket): boolean =>
	JSON.stringify(a) === JSON.stringify(b);

// The store over `boundary`'s storage and pipe, its links closing with the scope.
const makeStore = (
	boundary: Boundary,
): Effect.Effect<Store, never, Scope.Scope> =>
	Effect.gen(function* opened() {
		const links = yield* FiberMap.make<HostKey>();
		const run = yield* FiberMap.runtime(links)();
		const linked = new Map<HostKey, HostLink>();
		const listeners = new Set<() => void>();
		let view: View = { hosts: new Map(), route: Route.Pairing(), notice: null };

		const set = (next: View): void => {
			view = next;
			for (const listener of listeners) {
				listener();
			}
		};

		const withHosts = (hosts: View["hosts"]): View => ({
			...view,
			hosts,
			route: shown(boundary.fragment(), hosts),
		});

		const updated = (key: HostKey, update: Update): void => {
			const host = view.hosts.get(key);
			if (host === undefined) {
				return;
			}
			const next = applyUpdate(host, update);
			if (next.saved !== host.saved) {
				save(boundary.storage, { ticket: next.ticket, name: next.saved });
			}
			set({ ...view, hosts: new Map(view.hosts).set(key, next) });
		};

		// Updates from a link the host has since replaced or forgotten are dropped.
		const follow = (key: HostKey, ticket: Ticket): Effect.Effect<void> =>
			Effect.scoped(
				Effect.gen(function* following() {
					const current = yield* link(boundary.pipe, ticket, boundary.client);
					linked.set(key, current);
					yield* Effect.addFinalizer(() =>
						Effect.sync(() => {
							if (linked.get(key) === current) {
								linked.delete(key);
							}
						}),
					);
					yield* Stream.runForEach(current.updates, (update) =>
						Effect.sync(() => {
							if (linked.get(key) === current) {
								updated(key, update);
							}
						}),
					);
				}),
			);

		const adopt = (ticket: Ticket, name: string | null): HostKey => {
			const key = hostKey(ticket);
			const known = view.hosts.get(key);
			if (known !== undefined && sameTicket(known.ticket, ticket)) {
				return key;
			}
			const saved = known?.saved ?? name;
			save(boundary.storage, { ticket, name: saved });
			const host: PairedHost = {
				key,
				ticket,
				saved,
				status: disconnected,
				fold: known?.fold ?? emptyHost(),
			};
			set(withHosts(new Map(view.hosts).set(key, host)));
			run(key, follow(key, ticket));
			return key;
		};

		const followFragment = (): void => {
			const fragment = boundary.fragment();
			if (!isJoin(fragment)) {
				set({ ...view, route: shown(fragment, view.hosts) });
				return;
			}
			const joined = parseJoin(fragment);
			boundary.replace(
				fragmentOf(
					Result.isSuccess(joined) ? Route.Sessions() : Route.Pairing(),
				),
			);
			if (Result.isSuccess(joined)) {
				adopt(joined.success, null);
			}
			set({
				...withHosts(view.hosts),
				notice: Result.isFailure(joined) ? joined.failure : null,
			});
		};

		const show = (next: Route): void => {
			boundary.go(fragmentOf(next));
			followFragment();
		};

		const pair = (text: string): Result.Result<HostKey, TicketError> => {
			const parsed = parseJoin(text);
			if (Result.isFailure(parsed)) {
				return Result.fail(parsed.failure);
			}
			const key = adopt(parsed.success, null);
			set({ ...view, notice: null });
			show(Route.Sessions());
			return Result.succeed(key);
		};

		const forget = (key: HostKey): void => {
			linked.delete(key);
			Effect.runFork(FiberMap.remove(links, key));
			unsave(boundary.storage, key);
			set(
				withHosts(
					new Map([...view.hosts].filter(([paired]) => paired !== key)),
				),
			);
		};

		const send = (
			key: HostKey,
			body: Command,
		): Effect.Effect<CommandResult, SendError> =>
			Effect.suspend(
				() =>
					linked.get(key)?.send(body) ?? Effect.fail(SendError.NotConnected()),
			);

		const reconnect = (): void => {
			for (const current of linked.values()) {
				Effect.runFork(current.reconnect);
			}
		};

		for (const saved of restore(boundary.storage)) {
			adopt(saved.ticket, saved.name);
		}
		followFragment();

		return {
			view: (): View => view,
			subscribe: (changed: () => void): (() => void) => {
				listeners.add(changed);
				return (): void => {
					listeners.delete(changed);
				};
			},
			pair,
			forget,
			send,
			show,
			follow: followFragment,
			reconnect,
		};
	});

export {
	type Boundary,
	listed,
	makeStore,
	nameOf,
	type PairedHost,
	sessionOf,
	sessionsOf,
	type Store,
	type View,
};
