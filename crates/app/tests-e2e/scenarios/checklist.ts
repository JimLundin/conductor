import {
	boxOf,
	COMPLETED,
	isOpen,
	isWithinWidth,
	lastSegment,
	LIVE,
	type Run,
	squeeze,
	THUMB,
	tools,
} from "../harness.ts";

// The efforts the header offers once one is set: the model's, without the
// default.
const SET_EFFORTS = 5;
const ITEMS = 3;

// fixtures/e2e/claude-checklist-effort.jsonl: a turn whose three TodoWrite
// calls keep a checklist, then the effort set to low and a turn at it.
export const checklist = async (run: Run): Promise<void> => {
	const { page, out, dir, patience, log } = run;
	const { check, pane, settled, waitFor, openRow, startSession, send, text } =
		tools(run);
	await startSession("Claude Code", dir);
	const picker = pane.locator("select.effort-pick");
	check(
		(await picker.locator("option").allInnerTexts()).join(", ") ===
			"default effort, low effort, medium effort, high effort, xhigh effort, max effort",
		"the header offers the model's efforts, starting at its default",
	);
	check(
		(await pane.locator("details.agent-checklist").count()) === 0,
		"no checklist before the agent writes one",
	);
	await send("plan");
	await pane.locator(COMPLETED).waitFor({ timeout: patience });
	const checklist = pane.locator("details.agent-checklist");
	check(
		squeeze(await checklist.locator("summary").innerText()).trim() ===
			"Checklist 1 of 3 done",
		"the checklist counts what is done",
	);
	check(
		(
			await checklist
				.locator(".agent-checklist-item")
				.evaluateAll((items) =>
					items
						.filter((item) => item instanceof HTMLElement)
						.map(
							(item) =>
								`${item.getAttribute("data-status")}: ${item.innerText.trim()}`,
						),
				)
		).join(" | ") ===
			"completed: Write a.txt | pending: Write b.txt | pending: Write c.txt",
		"each item shows its status and text, in the agent's order",
	);
	check(
		(await checklist
			.locator(".agent-checklist-mark")
			.first()
			.getAttribute("aria-label")) === "Done",
		"a status is announced in words",
	);
	check(await isOpen(checklist), "the checklist starts open");
	check(
		(await pane.locator(".tool").count()) === 0,
		"the checklist calls show no tool cards",
	);
	check(await isWithinWidth(page), "no page overflow at phone width");
	const summary = await boxOf(checklist.locator("summary"));
	check(
		summary.height >= THUMB,
		`the checklist's summary is a thumb-sized target (${Math.round(summary.height)}px)`,
	);
	await settled();
	await page.screenshot({ path: out });
	log(`screenshot ${out}`);
	await picker.selectOption("low");
	await page.waitForFunction(
		(count) =>
			document.querySelectorAll("select.effort-pick option").length === count,
		SET_EFFORTS,
		{ timeout: patience },
	);
	check(
		(await picker.inputValue()) === "low",
		"the header shows the new effort once the host has logged it",
	);
	await send("pong");
	await pane.locator(`${COMPLETED} >> nth=1`).waitFor({ timeout: patience });
	check((await text(".reply")).endsWith("pong"), "the next turn ran");
	await checklist.locator("summary").tap();
	check(!(await isOpen(checklist)), "a tap folds the checklist away");
	await page.reload();
	await waitFor(LIVE);
	await openRow(lastSegment(dir));
	await pane.locator("details.agent-checklist").waitFor({ timeout: patience });
	check(
		(await pane.locator(".agent-checklist-item").count()) === ITEMS &&
			(await pane.locator("select.effort-pick").inputValue()) === "low",
		"the checklist and the effort replay after a reload",
	);
};
