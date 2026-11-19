import { isWithinWidth, type Run, tools } from "../harness.ts";

const PENDING_RESPONSE_MS = 250;
const TURNS = 3;

/** The middle turn's changes opened and closed from the keyboard, with focus
 * returned to the button that opened them. */
const changesByKeyboard = async (run: Run): Promise<void> => {
	const { page, patience } = run;
	const { check, pane } = tools(run);
	const turns = pane.locator(".turn-changes");
	const changesButton = turns
		.nth(1)
		.getByRole("button", { name: "Changes", exact: true });
	await changesButton.focus();
	await page.keyboard.press("Enter");
	const closeButton = turns
		.nth(1)
		.getByRole("button", { name: "Close changes", exact: true });
	await closeButton.waitFor({ timeout: patience });
	const wasLoading = (await turns.nth(1).innerText()).includes(
		"Loading changes…",
	);
	await closeButton.focus();
	await page.keyboard.press("Enter");
	await page.waitForFunction(
		(id) => document.activeElement?.id === id,
		await changesButton.getAttribute("id"),
	);
	let when = "after fast response";
	if (wasLoading) {
		when = "during loading";
	}
	check(true, `keyboard Close returns focus to Changes (${when})`);
	await page.waitForTimeout(PENDING_RESPONSE_MS);
	check(
		(await turns.nth(1).locator(".diff-panel").count()) === 0,
		"closed preview remains closed after pending response",
	);
	await page.keyboard.press("Enter");
};

/** The middle turn's diff, and an undo of it and every later turn. */
const undoMiddleTurn = async (run: Run): Promise<void> => {
	const { page, out, patience } = run;
	const { check, pane } = tools(run);
	const diff = pane.locator(".turn-changes").nth(1).locator(".diff-panel");
	await diff.waitFor({ timeout: patience });
	check(
		(await diff.locator(".undo-scope").innerText()).includes(
			"every later turn",
		),
		"full undo scope visible before action",
	);
	const preview = diff.locator(".diff-files");
	check(
		(
			await preview.evaluate((el) => getComputedStyle(el).overscrollBehavior)
		).includes("contain"),
		"diff scrolling is contained",
	);
	const patches = diff.locator(".diff-patch");
	if ((await patches.count()) > 0) {
		check(
			(await patches
				.first()
				.evaluate((el) => getComputedStyle(el).userSelect)) === "text",
			"patch text remains selectable",
		);
	}
	check(await isWithinWidth(page), "no page overflow at phone width");
	await page.screenshot({ path: out, fullPage: true });
	await diff
		.getByRole("button", { name: "Undo this turn", exact: true })
		.focus();
	await page.keyboard.press("Enter");
	await pane
		.getByRole("status")
		.filter({
			hasText: "Turn and all later file and conversation changes undone.",
		})
		.waitFor({ timeout: patience });
	await page.waitForFunction(() =>
		document.activeElement?.matches(
			".composer textarea:not(:disabled), .undo-status",
		),
	);
	check(true, "undo moves keyboard focus to surviving composer or status");
};

export const worktrees = async (run: Run): Promise<void> => {
	const { page, patience } = run;
	const { check, pane, waitFor } = tools(run);
	// Harness provides one idle session with three completed checkpointed turns.
	await page.locator(".session-row").first().tap();
	await pane
		.locator(".turn-changes")
		.nth(TURNS - 1)
		.waitFor({ timeout: patience });
	check(
		(await pane.locator(".user").count()) === TURNS,
		"three turns before undo",
	);
	check(
		(await pane.locator(".workspace-branch").innerText()).startsWith(
			"Branch: ",
		),
		"session branch context",
	);
	const directory = pane.locator(".workspace-directory");
	check(
		!(await directory.evaluate((el: HTMLDetailsElement) => el.open)),
		"working directory stays compact initially",
	);
	await directory.locator("summary").tap();
	check(
		(await directory.locator("code").innerText()).includes("/worktrees/"),
		"full working directory accessible through disclosure",
	);
	check(
		(await directory
			.locator("code")
			.evaluate((el) => getComputedStyle(el).userSelect)) === "text",
		"full working directory remains selectable",
	);
	await directory.locator("summary").tap();
	await changesByKeyboard(run);
	await undoMiddleTurn(run);
	check(
		(await pane.locator(".user").count()) === 1,
		"selected and later conversation turns removed",
	);
	check(
		(await pane.locator(".turn-changes").count()) === 1,
		"selected and later checkpoint controls removed",
	);
	await page.reload();
	await waitFor('header.status[data-state="Live"]');
	await page.locator(".session-row").first().tap();
	await pane.locator(".user").waitFor();
	check(
		(await pane.locator(".user").count()) === 1,
		"undo survives reconnect and durable replay",
	);
};
