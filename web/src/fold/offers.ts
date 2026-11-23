// What a session offers and shows, decided here so the views only render it.
import { Data, Match } from "effect";
import type { Session, SessionStatus } from "./session.ts";
import { isRequestOpen, type Item } from "./transcript.ts";

type Composer = "Open" | "Busy" | "Closed";

interface Controls {
	readonly composer: Composer;
	readonly interrupt: boolean;
	readonly continue: boolean;
	readonly finish: boolean;
}

type ListStatus = Data.TaggedEnum<{
	readonly Session: { readonly status: SessionStatus["_tag"] };
	readonly Requests: { readonly count: number };
}>;

const ListStatus = Data.taggedEnum<ListStatus>();

type EmptyTranscript = "Starting" | "Ready" | "Closed";

// The approvals and questions waiting on a person, the session's own and then its sub-agents'.
const openRequests = (session: Session): readonly Item[] =>
	[
		...session.items,
		...session.subagents.flatMap((subagent) => subagent.items),
	].filter(isRequestOpen);

const controls = (session: Session): Controls => {
	const status = session.status._tag;
	return {
		composer: Match.value(status).pipe(
			Match.withReturnType<Composer>(),
			Match.when("Idle", () => "Open"),
			Match.when("Closed", () => "Closed"),
			Match.orElse(() => "Busy"),
		),
		interrupt: status === "Running",
		continue: status === "Closed",
		finish:
			session.issue !== null &&
			(status === "Idle" || status === "Closed") &&
			(session.finish === "Ready" || session.finish === "Retrying"),
	};
};

// The list shows a session's open requests in place of its status.
const listStatus = (session: Session): ListStatus => {
	const count = openRequests(session).length;
	return count === 0
		? ListStatus.Session({ status: session.status._tag })
		: ListStatus.Requests({ count });
};

const emptyTranscript = (session: Session): EmptyTranscript | null => {
	if (session.items.length > 0) {
		return null;
	}
	return Match.value(session.status._tag).pipe(
		Match.withReturnType<EmptyTranscript>(),
		Match.when("Starting", () => "Starting"),
		Match.when("Closed", () => "Closed"),
		Match.orElse(() => "Ready"),
	);
};

const isLive = (item: Item | undefined): boolean =>
	item !== undefined &&
	(((item._tag === "Reply" || item._tag === "Reasoning") &&
		item.state === "Streaming") ||
		(item._tag === "Tool" && item.state._tag === "Running"));

// A running turn shows it is working, unless it waits on a person or its last item is live
// already: a reply or reasoning streaming in, or a tool call running.
const isWorking = (session: Session): boolean =>
	session.status._tag === "Running" &&
	openRequests(session).length === 0 &&
	!isLive(session.items.at(-1));

export {
	controls,
	emptyTranscript,
	isWorking,
	ListStatus,
	listStatus,
	openRequests,
};
