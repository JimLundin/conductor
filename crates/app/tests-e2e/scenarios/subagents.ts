import {
	boxOf,
	lastSegment,
	isWithinWidth,
	type Run,
	squeeze,
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
	await pane
		.locator('.turn-end[data-outcome="completed"]')
		.waitFor({ timeout: patience });
	const group = pane.locator("details.subagent");
	const isOpen = (): Promise<boolean> =>
		group.evaluate((details: HTMLDetailsElement) => details.open);
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
		squeeze(await group.locator("summary").innerText()).trim() ===
			"Run TodoWrite checklist task Finished",
		"the folded group names its description and status",
	);
	check(!(await isOpen()), "the group starts folded");
	check(
		!(await group.locator(".subagent-items").isVisible()),
		"its transcript is hidden while folded",
	);
	check(
		(await pane.locator("details.tasks").count()) === 0,
		"the sub-agent's checklist is not the session's tasks",
	);
	check(
		(await text(".items > .reply")) === "done",
		"the session's own reply shows once, outside the group",
	);
	const summary = await boxOf(group.locator("summary"));
	check(
		summary.height >= THUMB,
		`the group's summary is a thumb-sized target (${Math.round(summary.height)}px)`,
	);
	await group.locator("summary").tap();
	check(await isOpen(), "a tap opens the group");
	check(
		(await group.locator(".subagent-items .reply").innerText()) === "ok",
		"the open group shows the sub-agent's report",
	);
	check(await isWithinWidth(page), "no page overflow at phone width");
	await page.screenshot({ path: out });
	log(`screenshot ${out}`);
	await page.reload();
	await waitFor('header.status[data-state="Live"]');
	await openRow(lastSegment(dir));
	await pane.locator("details.subagent").waitFor({ timeout: patience });
	check(
		(await pane.locator("details.subagent").getAttribute("data-status")) ===
			"finished",
		"the sub-agent replays after a reload",
	);
};
