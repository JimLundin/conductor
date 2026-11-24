import { Effect } from "effect";
import { renderToStaticMarkup } from "react-dom/server";
import { expect, test } from "vitest";
import {
	created,
	exited,
	foldEvents,
	foldUpdates,
	id,
	ls,
	question,
	received,
	sessionOf,
	started,
	subagentStarted,
	subagentTool,
	user,
} from "../fold/examples.ts";
import type { Host } from "../fold/host.ts";
import { openRequests } from "../fold/offers.ts";
import { recordedFeed } from "../recorded-feeds.ts";
import type { Command, Decision, EventKind } from "../schemas/index.ts";
import {
	type Asked,
	answer,
	answersOf,
	decisions,
	message,
	notice,
	type Offer,
	type Send,
	sessionOffers,
} from "./commands.ts";
import { NewSession } from "./new-session.tsx";
import { SessionView } from "./session-view.tsx";
import { blank, choosable, effortsOf, startOf } from "./start.ts";

const session = id(1);

// A send that keeps what it was asked to send, so a test sees a render sent nothing.
interface Recording {
	readonly send: Send;
	readonly sent: () => readonly Command[];
}

const recording = (): Recording => {
	let sent: readonly Command[] = [];
	return {
		send: (body) =>
			Effect.sync(() => {
				sent = [...sent, body];
				return { Session: "Accepted" } as const;
			}),
		sent: () => sent,
	};
};

const view = (host: Host): string => {
	const { send, sent } = recording();
	const html = renderToStaticMarkup(
		<SessionView
			hostName="test-host"
			id={session}
			send={send}
			session={sessionOf(host, session)}
		/>,
	);
	expect(sent()).toStrictEqual([]);
	return html;
};

const offered = (offers: readonly Offer[], label: string): Command => {
	const offer = offers.find((one) => one.label === label);
	if (offer === undefined) {
		throw new Error(`nothing offered as ${label}`);
	}
	return offer.body;
};

const askedOf = (host: Host): Asked => {
	const [item] = openRequests(sessionOf(host, session));
	if (item?._tag !== "Tool" || item.approval._tag !== "Asked") {
		throw new Error("no approval is open");
	}
	return item.approval;
};

const button = (label: string): string => `type="button">${label}</button>`;

const turn = received(
	recordedFeed("a_claude_turn_streams_asks_and_finishes"),
	0,
);

// The recorded turn as the app folds it through its stored event at `seq`.
const turnThrough = (seq: number): Host =>
	foldUpdates(
		turn.filter(
			(update) =>
				update._tag !== "Feed" ||
				!("Event" in update.frame) ||
				update.frame.Event.seq <= seq,
		),
	);

// The session's events, numbered as its host stores them.
const onSession = (kinds: readonly EventKind[]): Host =>
	foldEvents(kinds.map((kind) => [1, kind] as const));

const directory = "/conductor/target/tmp/conductor-e2e-0-approve";
const ready = [created(directory), "SessionReady"] as const;

test("a new session starts on the chosen instance in a directory, and opens ready for a message", () => {
	const before = turnThrough(0);
	const draft = { ...blank, host: "laptop", instance: "claude", directory };
	const form = renderToStaticMarkup(
		<NewSession
			hosts={[
				{
					key: "laptop",
					name: "test-host",
					host: before,
					send: recording().send,
				},
			]}
			initial="laptop"
			started={(): void => undefined}
		/>,
	);
	expect(form).toContain(
		'<option value="laptop" selected="">test-host</option>',
	);
	expect(form).toContain('<option value="claude">Claude Code</option>');
	expect(form).not.toContain("Codex");
	expect(startOf(before, draft)).toStrictEqual({
		Session: {
			Start: {
				instance: { account: "Own", provider: "claude" },
				at: { Directory: directory },
				model: null,
				effort: null,
			},
		},
	});
	const after = turnThrough(4);
	expect(view(after)).toContain("Send a message to start.");
	expect(view(after)).toContain(
		'<button class="primary" type="submit">Send</button>',
	);
});

test("a new session takes a model and an effort the instance offers, or a project", () => {
	const before = turnThrough(0);
	const [claude] = choosable(before);
	expect(claude?.models.choices.map((choice) => choice.id)).toStrictEqual([
		"big",
		"small",
	]);
	expect(
		effortsOf(claude?.models ?? { choices: [], default: null }, null),
	).toStrictEqual(["Low", "High"]);
	const draft = { ...blank, instance: "claude", model: "small", effort: "Low" };
	expect(startOf(before, { ...draft, project: "0twr04e5" })).toStrictEqual({
		Session: {
			Start: {
				instance: { account: "Own", provider: "claude" },
				at: { Project: "0twr04e5" },
				model: "small",
				effort: "Low",
			},
		},
	});
	expect(startOf(before, draft)).toBeNull();
	expect(
		startOf(before, { ...draft, instance: "codex", directory }),
	).toBeNull();
});

test("a message is sent, and the transcript shows it, its tool card and the approval it waits on", () => {
	expect(view(turnThrough(4))).toContain('<textarea aria-label="Message"');
	expect(message(session, "create a.txt")).toStrictEqual({
		Session: { SendMessage: { session, text: "create a.txt" } },
	});
	const html = view(turnThrough(8));
	expect(html).toContain('<p class="bubble">create a.txt</p>');
	expect(html).toContain("<code>/tmp/s1-scratch/a.txt</code>");
	expect(html).toContain('<p class="approval open">Waiting for approval</p>');
	expect(html).toContain(button("Approve"));
	expect(html).toContain(button("Deny"));
	expect(html).not.toContain("Approve for session");
	expect(html).toContain('disabled="" type="submit">Send</button>');
});

test("an approval is sent, and its card, the streamed reply and the finished turn follow", () => {
	expect(
		offered(decisions(session, askedOf(turnThrough(8))), "Approve"),
	).toStrictEqual({
		Session: { Resolve: { session, request: 1, decision: "Approve" } },
	});
	const html = view(turnThrough(13));
	expect(html).toContain("Approved by test-client");
	expect(html).toContain('data-status="Succeeded"');
	expect(html).toContain(
		"<p><span>I created </span><code>a.txt</code><span> in </span><code>/tmp/s1-scratch</code>",
	);
	expect(html).toContain("Turn completed");
	expect(html).not.toContain("Waiting for approval");
});

test("a rejected command tells the person why", () => {
	expect(
		notice({
			Session: { Refused: { AlreadyResolved: { by: "test-client" } } },
		}),
	).toBe("The host refused: Already resolved: test-client.");
	expect(notice({ Session: { Refused: "TurnInProgress" } })).toBe(
		"The host refused: Turn in progress.",
	);
	expect(notice({ Session: "Accepted" })).toBeNull();
});

const askedForSession = {
	RequestOpened: {
		request: 1,
		call: "toolu_1",
		tool: ls,
		reason: "to see the files",
		for_session: "Bash(ls)",
	},
} as const satisfies EventKind;

test.each([
	[
		"Approve for session",
		"ApproveForSession",
		"Approved for the session by phone",
	],
	["Deny", "Deny", "Denied by phone"],
] as const)(
	"%s is offered when the agent offers it, and the card follows the decision",
	(label, decision: Decision, shown) => {
		const asking = [
			...ready,
			user("look"),
			started("toolu_1"),
			askedForSession,
		];
		const before = onSession(asking);
		expect(view(before)).toContain(button(label));
		expect(view(before)).toContain("For the session: Bash(ls)");
		expect(offered(decisions(session, askedOf(before)), label)).toStrictEqual({
			Session: { Resolve: { session, request: 1, decision } },
		});
		const after = onSession([
			...asking,
			{
				RequestResolved: {
					request: 1,
					resolution: { Decided: { by: "phone", decision } },
				},
			},
		]);
		expect(view(after)).toContain(shown);
	},
);

test("a question is answered with the options picked, and the card shows the answer", () => {
	const asking = [...ready, user("paint it"), question(1)];
	const before = onSession(asking);
	expect(view(before)).toContain("<legend>Color</legend><p>Which color?</p>");
	expect(view(before)).toContain('title="Cool" type="button">Blue</button>');
	expect(
		answer(session, 1, answersOf([{ labels: ["Blue"], typed: "" }])),
	).toStrictEqual({
		Session: { Answer: { session, request: 1, answers: [["Blue"]] } },
	});
	const after = onSession([
		...asking,
		{
			RequestResolved: {
				request: 1,
				resolution: { Answered: { by: "phone", answers: [["Blue"]] } },
			},
		},
	]);
	expect(view(after)).toContain("Answered by phone: Blue");
});

test("an answer holds the options picked, then the text typed instead", () => {
	expect(
		answersOf([
			{ labels: ["Blue", "Red"], typed: "  teal " },
			{ labels: [], typed: "" },
		]),
	).toStrictEqual([["Blue", "Red", "teal"], []]);
});

test("a running turn offers Stop, which interrupts it", () => {
	const running = [...ready, user("look"), started("toolu_1")];
	const before = onSession(running);
	expect(view(before)).toContain(button("Stop"));
	expect(
		offered(sessionOffers(session, sessionOf(before, session)), "Stop"),
	).toStrictEqual({ Session: { Interrupt: { session } } });
	const after = onSession([
		...running,
		{ TurnCompleted: { outcome: "Interrupted" } },
	]);
	expect(view(after)).toContain('<p class="marker stopped">Interrupted</p>');
	expect(view(after)).toContain('data-status="Stopped"');
});

test("a closed session offers Continue, and is ready again once its agent starts", () => {
	const closed = [...ready, user("look"), exited("1")];
	const before = onSession(closed);
	expect(view(before)).toContain(button("Continue"));
	expect(view(before)).toContain("Closed: it exited with 1");
	expect(
		offered(sessionOffers(session, sessionOf(before, session)), "Continue"),
	).toStrictEqual({ Session: { Continue: { session } } });
	const after = onSession([...closed, "SessionReady"]);
	expect(view(after)).toContain("Continued after it exited with 1");
});

test("a session on an issue offers Finish, and shows how finishing went", () => {
	const onIssue = [
		created(directory),
		{ OnIssue: { issue: "0twtnsfs" } },
		"SessionReady",
	] as const;
	const before = onSession(onIssue);
	expect(view(before)).toContain(button("Finish"));
	expect(
		offered(sessionOffers(session, sessionOf(before, session)), "Finish"),
	).toStrictEqual({ Session: { Finish: { session } } });
	const after = onSession([
		...onIssue,
		"FinishStarted",
		{
			SessionFinished: {
				finished: { Landed: { commits: 2, kept: "Local" } },
			},
		},
	]);
	expect(view(after)).toContain("Finishing on its issue");
	expect(view(after)).toContain("Finished: 2 commits landed, kept locally");
});

test("a sub-agent shows under the tool call that started it, with its own tool cards", () => {
	const host = foldEvents([
		[1, created(directory)],
		[1, "SessionReady"],
		[1, user("look around")],
		[1, started("toolu_9")],
		[1, subagentStarted("scout", "toolu_9")],
		[
			1,
			subagentTool("scout", "toolu_10", {
				Started: { tool: { Command: { command: "rg todo" } } },
			}),
		],
	]);
	const html = view(host);
	expect(html).toMatch(
		/<article class="tool running"[^>]*>(?:(?!<\/article>).)*look for scout(?:(?!<\/article>).)*rg todo/su,
	);
});

test("a reply shows streaming while its deltas arrive", () => {
	const host = foldEvents([
		[1, created(directory)],
		[1, "SessionReady"],
		[1, user("hi")],
		[1, { Agent: { AssistantDelta: { text: "Hel" } } }],
		[1, { Agent: { AssistantDelta: { text: "lo" } } }],
	]);
	expect(view(host)).toContain(
		'<div class="reply streaming"><div class="markdown"><p><span>Hello</span>',
	);
});
