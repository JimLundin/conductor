import { expect, test } from "vitest";
import type { EventKind, Resolution } from "../proto/index.ts";
import {
	created,
	delta,
	finished,
	foldEvents,
	id,
	opened,
	question,
	sessionOf,
	started,
	user,
	withdrawn,
} from "./examples.ts";
import {
	emptyTranscript,
	isWorking,
	ListStatus,
	listStatus,
	openRequests,
} from "./offers.ts";
import { RequestState } from "./transcript.ts";

const cases = [
	[[], false],
	[[user("hi")], true],
	[[user("hi"), delta("Hel")], false],
	[[user("hi"), delta("Hel"), { AssistantMessage: { text: "Hello" } }], true],
	[[user("hi"), { ReasoningDelta: { text: "Hm" } }], false],
	[[user("hi"), started("toolu_1")], false],
	[[user("hi"), started("toolu_1"), finished("toolu_1")], true],
	[[user("hi"), { TurnCompleted: { outcome: "Completed" } }], false],
	// An open approval after a parallel tool call finished, and after a commentary reply.
	[
		[
			user("hi"),
			started("toolu_1"),
			started("toolu_2"),
			finished("toolu_2"),
			opened(1),
		],
		false,
	],
	[
		[
			user("hi"),
			started("toolu_1"),
			{ AssistantMessage: { text: "I need to write a file." } },
			opened(1),
		],
		false,
	],
] as const satisfies readonly (readonly [readonly EventKind[], boolean])[];

test.each(cases)(
	"a running turn shows it is working until something live ends the transcript (%j)",
	(events, shown) => {
		const host = foldEvents(
			[created("/a"), "SessionReady" as const, ...events].map(
				(kind) => [1, kind] as const,
			),
		);
		expect(isWorking(sessionOf(host, id(1)))).toBe(shown);
	},
);

test("the list shows the open requests in place of the session's status", () => {
	const host = foldEvents([
		[1, created("/a")],
		[1, opened(1)],
		[1, opened(2)],
		[1, opened(3)],
		[1, { RequestResolved: { request: 2, resolution: "Withdrawn" } }],
		[2, created("/b")],
	]);
	expect(listStatus(sessionOf(host, id(1)))).toStrictEqual(
		ListStatus.Requests({ count: 2 }),
	);
	expect(listStatus(sessionOf(host, id(2)))).toStrictEqual(
		ListStatus.Session({ status: "Starting" }),
	);
	expect(emptyTranscript(sessionOf(host, id(2)))).toBe("Starting");
	expect(emptyTranscript(sessionOf(host, id(1)))).toBeNull();
});

test("a question waits on a person until answered or the turn ends", () => {
	const answered: Resolution = {
		Answered: { answers: [["Blue"]], by: "phone" },
	};
	const open = foldEvents([
		[1, created("/a")],
		[1, "SessionReady"],
		[1, user("ask me")],
		[1, question(1)],
		[1, question(2)],
		[1, opened(3)],
	]);
	expect(openRequests(sessionOf(open, id(1)))).toHaveLength(3);
	expect(isWorking(sessionOf(open, id(1)))).toBe(false);
	const later = foldEvents([
		[1, created("/a")],
		[1, question(1)],
		[1, question(2)],
		[1, { RequestResolved: { request: 1, resolution: answered } }],
		[1, { TurnCompleted: { outcome: "Interrupted" } }],
	]);
	expect(
		sessionOf(later, id(1)).items.flatMap((item) =>
			item._tag === "Question" ? [[item.request, item.state]] : [],
		),
	).toStrictEqual([
		[1, RequestState.Resolved({ resolution: answered })],
		[2, withdrawn],
	]);
	expect(openRequests(sessionOf(later, id(1)))).toStrictEqual([]);
});
