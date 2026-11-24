import { Effect } from "effect";
import { renderToStaticMarkup } from "react-dom/server";
import { expect, test } from "vitest";
import {
	created,
	exited,
	foldEvents,
	ls,
	question,
	sessionOf,
	started,
	subagentStarted,
	subagentTool,
	user,
} from "../fold/examples.ts";
import type { Host } from "../fold/host.ts";
import { openRequests } from "../fold/offers.ts";
import type { SendError } from "../link/command.ts";
import type {
	Command,
	CommandResult,
	Decision,
	EventKind,
} from "../schemas/index.ts";
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
import { framed, played, recordedSpan, session } from "./playback.ts";
import { SessionView } from "./session-view.tsx";
import { blank, choosable, effortsOf, startOf } from "./start.ts";

const unsent: Send = () =>
	Effect.die("the view sent a command while rendering");

const view = (host: Host): string =>
	renderToStaticMarkup(
		<SessionView
			hostName="test-host"
			id={session}
			send={unsent}
			session={sessionOf(host, session)}
		/>,
	);

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

// The result the host gave, not a failure to send.
const taken = (result: CommandResult | SendError): CommandResult => {
	if (typeof result === "object" && "_tag" in result) {
		throw new Error(`the command was not sent: ${result._tag}`);
	}
	return result;
};

const button = (label: string): string => `type="button">${label}</button>`;

const directory = "/conductor/target/tmp/conductor-e2e-0-approve";
const ready = [created(directory), "SessionReady"] as const;

test("a new session starts on the chosen instance in a directory, and opens ready for a message", async () => {
	const draft = { ...blank, host: "laptop", instance: "claude", directory };
	const { before, commands, result, after } = await played(
		{
			feed: recordedSpan(0, 0),
			result: { Session: { Started: { session } } },
			answer: recordedSpan(1, 4),
		},
		(host) => startOf(host, draft) ?? { Home: "Sync" },
	);
	const form = renderToStaticMarkup(
		<NewSession
			hosts={[{ key: "laptop", name: "test-host", host: before, send: unsent }]}
			initial="laptop"
			started={(): void => undefined}
		/>,
	);
	expect(form).toContain(
		'<option value="laptop" selected="">test-host</option>',
	);
	expect(form).toContain('<option value="claude">Claude Code</option>');
	expect(form).not.toContain("Codex");
	expect(commands).toStrictEqual([
		{
			Session: {
				Start: {
					instance: { account: "Own", provider: "claude" },
					at: { Directory: directory },
					model: null,
					effort: null,
				},
			},
		},
	]);
	expect(result).toStrictEqual({ Session: { Started: { session } } });
	expect(view(after)).toContain("Send a message to start.");
	expect(view(after)).toContain(
		'<button class="primary" type="submit">Send</button>',
	);
});

test("a new session takes a model and an effort the instance offers, or a project", async () => {
	const { before } = await played(
		{ feed: recordedSpan(0, 0), result: { Session: "Accepted" }, answer: [] },
		() => ({ Home: "Sync" }),
	);
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

test("a message is sent, and the transcript shows it, its tool card and the approval it waits on", async () => {
	const { before, commands, after } = await played(
		{
			feed: recordedSpan(0, 4),
			result: { Session: "Accepted" },
			answer: recordedSpan(5, 8),
		},
		() => message(session, "create a.txt"),
	);
	expect(view(before)).toContain('<textarea aria-label="Message"');
	expect(commands).toStrictEqual([
		{ Session: { SendMessage: { session, text: "create a.txt" } } },
	]);
	const html = view(after);
	expect(html).toContain('<p class="bubble">create a.txt</p>');
	expect(html).toContain("<code>/tmp/s1-scratch/a.txt</code>");
	expect(html).toContain('<p class="approval open">Waiting for approval</p>');
	expect(html).toContain(button("Approve"));
	expect(html).toContain(button("Deny"));
	expect(html).not.toContain("Approve for session");
	expect(html).toContain('disabled="" type="submit">Send</button>');
});

test("an approval is sent, and its card, the streamed reply and the finished turn follow", async () => {
	const { commands, after } = await played(
		{
			feed: recordedSpan(0, 8),
			result: { Session: "Accepted" },
			answer: recordedSpan(9, 13),
		},
		(host) => offered(decisions(session, askedOf(host)), "Approve"),
	);
	expect(commands).toStrictEqual([
		{ Session: { Resolve: { session, request: 1, decision: "Approve" } } },
	]);
	const html = view(after);
	expect(html).toContain("Approved by test-client");
	expect(html).toContain('data-status="Succeeded"');
	expect(html).toContain(
		"<p><span>I created </span><code>a.txt</code><span> in </span><code>/tmp/s1-scratch</code>",
	);
	expect(html).toContain("Turn completed");
	expect(html).not.toContain("Waiting for approval");
	expect(html).not.toContain(button("Approve"));
});

test("a rejected command tells the person why", async () => {
	const { result } = await played(
		{
			feed: recordedSpan(0, 8),
			result: {
				Session: { Refused: { AlreadyResolved: { by: "test-client" } } },
			},
			answer: [],
		},
		(host) => offered(decisions(session, askedOf(host)), "Approve"),
	);
	expect(notice(taken(result))).toBe(
		"The host refused: Already resolved: test-client.",
	);
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
	async (label, decision: Decision, shown) => {
		const { before, commands, after } = await played(
			{
				feed: framed(1, [
					...ready,
					user("look"),
					started("toolu_1"),
					askedForSession,
				]),
				result: { Session: "Accepted" },
				answer: framed(6, [
					{
						RequestResolved: {
							request: 1,
							resolution: { Decided: { by: "phone", decision } },
						},
					},
				]),
			},
			(host) => offered(decisions(session, askedOf(host)), label),
		);
		expect(view(before)).toContain(button(label));
		expect(view(before)).toContain("For the session: Bash(ls)");
		expect(commands).toStrictEqual([
			{ Session: { Resolve: { session, request: 1, decision } } },
		]);
		expect(view(after)).toContain(shown);
		expect(view(after)).not.toContain(button(label));
	},
);

test("a question is answered with the options picked, and the card shows the answer", async () => {
	const { before, commands, after } = await played(
		{
			feed: framed(1, [...ready, user("paint it"), question(1)]),
			result: { Session: "Accepted" },
			answer: framed(5, [
				{
					RequestResolved: {
						request: 1,
						resolution: { Answered: { by: "phone", answers: [["Blue"]] } },
					},
				},
			]),
		},
		() => answer(session, 1, answersOf([{ labels: ["Blue"], typed: "" }])),
	);
	expect(view(before)).toContain("<legend>Color</legend><p>Which color?</p>");
	expect(view(before)).toContain('title="Cool" type="button">Blue</button>');
	expect(commands).toStrictEqual([
		{ Session: { Answer: { session, request: 1, answers: [["Blue"]] } } },
	]);
	expect(view(after)).toContain("Answered by phone: Blue");
	expect(view(after)).not.toContain("<legend>Color</legend>");
});

test("an answer holds the options picked, then the text typed instead", () => {
	expect(
		answersOf([
			{ labels: ["Blue", "Red"], typed: "  teal " },
			{ labels: [], typed: "" },
		]),
	).toStrictEqual([["Blue", "Red", "teal"], []]);
});

test("a running turn offers Stop, which interrupts it", async () => {
	const { before, commands, after } = await played(
		{
			feed: framed(1, [...ready, user("look"), started("toolu_1")]),
			result: { Session: "Accepted" },
			answer: framed(5, [{ TurnCompleted: { outcome: "Interrupted" } }]),
		},
		(host) => offered(sessionOffers(session, sessionOf(host, session)), "Stop"),
	);
	expect(view(before)).toContain(button("Stop"));
	expect(commands).toStrictEqual([{ Session: { Interrupt: { session } } }]);
	expect(view(after)).toContain('<p class="marker stopped">Interrupted</p>');
	expect(view(after)).toContain('data-status="Stopped"');
	expect(view(after)).not.toContain(button("Stop"));
});

test("a closed session offers Continue, and is ready again once its agent starts", async () => {
	const { before, commands, after } = await played(
		{
			feed: framed(1, [...ready, user("look"), exited("1")]),
			result: { Session: "Accepted" },
			answer: framed(5, ["SessionReady"]),
		},
		(host) =>
			offered(sessionOffers(session, sessionOf(host, session)), "Continue"),
	);
	expect(view(before)).toContain(button("Continue"));
	expect(view(before)).toContain("Closed: it exited with 1");
	expect(commands).toStrictEqual([{ Session: { Continue: { session } } }]);
	expect(view(after)).toContain("Continued after it exited with 1");
	expect(view(after)).not.toContain(button("Continue"));
});

test("a session on an issue offers Finish, and shows how finishing went", async () => {
	const { before, commands, after } = await played(
		{
			feed: framed(1, [
				created(directory),
				{ OnIssue: { issue: "0twtnsfs" } },
				"SessionReady",
			]),
			result: { Session: "Accepted" },
			answer: framed(4, [
				"FinishStarted",
				{
					SessionFinished: {
						finished: { Landed: { commits: 2, kept: "Local" } },
					},
				},
			]),
		},
		(host) =>
			offered(sessionOffers(session, sessionOf(host, session)), "Finish"),
	);
	expect(view(before)).toContain(button("Finish"));
	expect(commands).toStrictEqual([{ Session: { Finish: { session } } }]);
	expect(view(after)).toContain("Finishing on its issue");
	expect(view(after)).toContain("Finished: 2 commits landed, kept locally");
	expect(view(after)).not.toContain(button("Finish"));
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
