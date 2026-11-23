import { expect, test } from "vitest";
import {
	asked,
	created,
	delta,
	exited,
	finished,
	foldEvents,
	id,
	ls,
	opened,
	reasoned,
	sessionOf,
	started,
	withdrawn,
} from "./examples.ts";
import { directories, recent } from "./host.ts";
import { openRequests } from "./offers.ts";
import { SessionStatus } from "./session.ts";
import { Approval, Item, ToolState } from "./transcript.ts";

test("the end of a turn or a session stops its tools and withdraws its requests", () => {
	const host = foldEvents([
		[1, created("/a")],
		[1, started("toolu_1")],
		[1, opened(1)],
		[1, started("toolu_2")],
		[1, finished("toolu_2")],
		[1, { TurnCompleted: { outcome: "Interrupted" } }],
		[2, created("/b")],
		[2, opened(3)],
		[2, exited("1")],
	]);
	expect(sessionOf(host, id(1)).items).toStrictEqual([
		asked("toolu_1", 1, ToolState.Stopped(), withdrawn),
		Item.Tool({
			call: "toolu_2",
			tool: ls,
			state: ToolState.Finished({ outcome: "Succeeded" }),
			approval: Approval.NotAsked(),
		}),
		Item.TurnCompleted({ outcome: "Interrupted" }),
	]);
	expect(sessionOf(host, id(2)).items).toStrictEqual([
		asked("toolu_3", 3, ToolState.Stopped(), withdrawn),
	]);
	expect(openRequests(sessionOf(host, id(1)))).toStrictEqual([]);
	expect(openRequests(sessionOf(host, id(2)))).toStrictEqual([]);
});

test("the list puts the latest activity first and suggests each directory once", () => {
	const host = foldEvents([
		[1, created("/a")],
		[2, created("/b")],
		[3, created("/a")],
		[2, "SessionReady"],
		[1, delta("x")],
	]);
	expect(recent(host)).toStrictEqual([id(2), id(3), id(1)]);
	expect(directories(host)).toStrictEqual(["/b", "/a"]);
});

test("a removed session leaves the host with its later events", () => {
	const host = foldEvents([
		[1, created("/a")],
		[2, created("/b")],
		[1, { SessionRemoved: { kept: null } }],
		[1, "SessionReady"],
	]);
	expect(recent(host)).toStrictEqual([id(2)]);
});

test("reasoning streams into one item that its text replaces, and a continue shows its close", () => {
	const host = foldEvents([
		[1, created("/a")],
		[1, "SessionReady"],
		[1, reasoned("Look")],
		[1, reasoned("ing")],
		[1, { Agent: { Reasoning: { text: "Looking." } } }],
		[1, reasoned("Again")],
		[1, delta("Hi")],
		[1, exited("9")],
		[1, "SessionReady"],
	]);
	expect(sessionOf(host, id(1)).items).toStrictEqual([
		Item.Reasoning({ text: "Looking.", state: "Done" }),
		Item.Reasoning({ text: "Again", state: "Done" }),
		Item.Reply({ text: "Hi", state: "Done" }),
		Item.Continued({ reason: { Exited: { status: "9" } } }),
	]);
	expect(sessionOf(host, id(1)).status).toStrictEqual(SessionStatus.Idle());
});

test("a reply and reasoning append every delta", () => {
	const host = foldEvents([
		[1, created("/a")],
		[1, reasoned("One")],
		[1, reasoned(" two")],
		[1, reasoned(" three")],
		[1, delta("Four")],
		[1, delta(" five")],
		[1, delta(" six")],
	]);
	expect(sessionOf(host, id(1)).items).toStrictEqual([
		Item.Reasoning({ text: "One two three", state: "Done" }),
		Item.Reply({ text: "Four five six", state: "Streaming" }),
	]);
});

test("a close settles the reasoning or reply that was streaming", () => {
	const host = foldEvents([
		[1, created("/a")],
		[1, "SessionReady"],
		[1, reasoned("Look")],
		[1, exited("9")],
		[2, created("/b")],
		[2, "SessionReady"],
		[2, delta("Hel")],
		[2, exited("9")],
	]);
	expect(sessionOf(host, id(1)).items).toStrictEqual([
		Item.Reasoning({ text: "Look", state: "Done" }),
	]);
	expect(sessionOf(host, id(2)).items).toStrictEqual([
		Item.Reply({ text: "Hel", state: "Done" }),
	]);
});

test("a model change shows in the header and the transcript", () => {
	const reason = "Model changed to gpt: small is not offered";
	const host = foldEvents([
		[1, created("/a")],
		[1, "SessionReady"],
		[1, exited("9")],
		[1, { ModelChanged: { model: "gpt", effort: null, reason } }],
	]);
	expect(sessionOf(host, id(1)).model).toBe("gpt");
	expect(sessionOf(host, id(1)).items).toStrictEqual([
		Item.ModelChanged({ reason }),
	]);
});

test("each change replaces the checklist, and the effort follows its last change", () => {
	const later = [{ text: "write a.txt", status: "Completed" }] as const;
	const host = foldEvents([
		[1, created("/a")],
		[1, "SessionReady"],
		[
			1,
			{
				ChecklistChanged: {
					checklist: [
						{ text: "write a.txt", status: "InProgress" },
						{ text: "write b.txt", status: "Pending" },
					],
				},
			},
		],
		[1, { ChecklistChanged: { checklist: later } }],
		[1, { EffortChanged: { effort: "Low" } }],
	]);
	expect(sessionOf(host, id(1)).checklist).toStrictEqual(later);
	expect(sessionOf(host, id(1)).effort).toBe("Low");
	expect(sessionOf(host, id(1)).items).toStrictEqual([]);
});
