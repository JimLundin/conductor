import {
	boxOf,
	COMPLETED,
	isOpen,
	isWithinWidth,
	lastSegment,
	LIVE,
	type Run,
	THUMB,
	tools,
} from "../harness.ts";

// fixtures/e2e/claude-subagent.jsonl: the main agent's Agent call runs a
// sub-agent, which keeps a checklist of its own and reports "ok".
export const subagents = async (run: Run): Promise<void> => {
	const { page, out, dir, patience, log } = run;
	const { check, pane, waitFor, openRow, startSession, send, text } =
		tools(run);
	await startSession("Claude Code", dir);
	await send("delegate");
	await pane.locator(COMPLETED).waitFor({ timeout: patience });
	const group = pane.locator("details.subagent");
	const heading = group.locator(":scope > summary");
	check((await group.count()) === 1, "one sub-agent group");
	check(
		(await text(".tool .tool-name")) === "Agent",
		"the call that started it shows as a tool call",
	);
	check(
		(await group.evaluate((details) =>
			details.previousElementSibling?.classList.contains("tool"),
		)) === true,
		"the group hangs right under the call that started it",
	);
	check(
		(await group.getAttribute("data-status")) === "finished",
		"the group carries its status",
	);
	check(
		(await heading.locator(":scope > span").allInnerTexts()).join(" ") ===
			"Run TodoWrite checklist task general-purpose Finished",
		"the folded group names its description, type and status",
	);
	check(!(await isOpen(group)), "the group starts folded");
	check(
		!(await group.locator(".subagent-items").isVisible()),
		"its transcript is hidden while folded",
	);
	check(
		(await pane.locator(":scope > details.tasks").count()) === 0,
		"the sub-agent's checklist is not the session's tasks",
	);
	check(
		(await text(".items > .reply")) === "done",
		"the session's own reply shows once, outside the group",
	);
	const summary = await boxOf(heading);
	check(
		summary.height >= THUMB,
		`the group's summary is a thumb-sized target (${Math.round(summary.height)}px)`,
	);
	await heading.tap();
	check(await isOpen(group), "a tap opens the group");
	check(
		(await group.locator('.tasks .task[data-status="completed"]').count()) ===
			2,
		"the opened group shows both completed checklist steps",
	);
	check(
		(await group.locator(".subagent-items .reply").innerText()) === "ok",
		"the open group shows the sub-agent's report",
	);
	check(await isWithinWidth(page), "no page overflow at phone width");
	await page.screenshot({ path: out });
	log(`screenshot ${out}`);
	await page.reload();
	await waitFor(LIVE);
	await openRow(lastSegment(dir));
	await pane.locator("details.subagent").waitFor({ timeout: patience });
	check(
		(await group.getAttribute("data-status")) === "finished",
		"the sub-agent replays after a reload",
	);
	await heading.tap();
	check(
		(await group.locator('.tasks .task[data-status="completed"]').count()) ===
			2,
		"the checklist replays after a reload",
	);
};
