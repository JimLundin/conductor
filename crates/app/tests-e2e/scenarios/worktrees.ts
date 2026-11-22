import { isWithinWidth, LIVE, type Run, tools } from "../harness.ts";

const PENDING_RESPONSE_MS = 250;
const TURNS = 3;

/** Dismiss a host-rejected preview and restore focus to its trigger. */
const dismissFailedChanges = async (run: Run): Promise<void> => {
	const { page, patience } = run;
	const { pane } = tools(run);
	const turn = pane.locator(".turn-changes").nth(1);
	const changes = turn.getByRole("button", { name: "Changes", exact: true });
	await changes.press("Enter");
	const alert = turn.getByRole("alert");
	await alert.waitFor({ timeout: patience });
	await turn
		.getByRole("button", { name: "Close changes", exact: true })
		.press("Enter");
	await alert.waitFor({ state: "detached", timeout: patience });
	await page.waitForFunction(
		(id) => document.activeElement?.id === id,
		await changes.getAttribute("id"),
	);
};

/** Retry the middle turn's preview from the restored focus, then close it. */
const changesByKeyboard = async (run: Run): Promise<void> => {
	const { page, patience } = run;
	const { check, pane } = tools(run);
	const turns = pane.locator(".turn-changes");
	const changesButton = turns
		.nth(1)
		.getByRole("button", { name: "Changes", exact: true });
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

const middleTurnChanges = async (run: Run): Promise<void> => {
	const { page, out, patience } = run;
	const { check, pane } = tools(run);
	const diff = pane.locator(".turn-changes").nth(1).locator(".diff-panel");
	await diff.waitFor({ timeout: patience });
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
};

const unchangedTurn = async (run: Run): Promise<void> => {
	const { patience } = run;
	const { check, pane, send } = tools(run);
	const turns = pane.locator(".turn-changes");
	const changed = turns.nth(1);
	const unchanged = turns.nth(TURNS);
	await send("turn four without file changes");
	await unchanged.waitFor({ timeout: patience });
	const changedId = await turns
		.nth(TURNS - 1)
		.locator(".show-changes")
		.getAttribute("id");
	const unchangedId = await unchanged
		.locator(".show-changes")
		.getAttribute("id");
	check(changedId !== unchangedId, "shared commit has distinct turn controls");
	await unchanged.getByRole("button", { name: "Changes", exact: true }).tap();
	await unchanged
		.getByText("No file changes in this turn.", { exact: true })
		.waitFor({ timeout: patience });
	check(
		(await changed.locator(".diff-panel").count()) === 1,
		"opening the unchanged turn preserves the previous preview",
	);
};

export const worktrees = async (run: Run): Promise<void> => {
	const { page, patience } = run;
	const { check, pane, waitFor } = tools(run);
	await page.locator(".session-row").first().tap();
	await pane
		.locator(".turn-changes")
		.nth(TURNS - 1)
		.waitFor({ timeout: patience });
	check(
		(await pane.locator(".user").count()) === TURNS,
		"three recorded turns",
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
	await dismissFailedChanges(run);
	await changesByKeyboard(run);
	await middleTurnChanges(run);
	await unchangedTurn(run);
	check(
		(await pane.locator(".user").count()) === TURNS + 1,
		"viewing diffs keeps every conversation turn",
	);
	check(
		(await pane.locator(".turn-changes").count()) === TURNS + 1,
		"each turn keeps its changes control",
	);
	await page.reload();
	await waitFor(LIVE);
	await page.locator(".session-row").first().tap();
	await pane.locator(".turn-changes").nth(TURNS).waitFor();
	check(
		(await pane.locator(".user").count()) === TURNS + 1,
		"all turns survive reconnect and durable replay",
	);
};
