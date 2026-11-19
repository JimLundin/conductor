import { type Run, squeeze, tools } from "../harness.ts";

const WHAT_CODEX_DOES =
	"Codex runs commands and edits files in this folder and in /tmp without asking. It asks before writing anywhere else, changing .git, or using the network.";

// fixtures/e2e/codex-two-threads.jsonl: two threads start, then a turn on
// each; the first asks to run a command and is approved, the second is
// declined.
export const codex = async (run: Run): Promise<void> => {
	const { page, out, dir, patience, log } = run;
	const { check, pane, startSession, openRow, send, text } = tools(run);
	const open = pane.locator('.request[data-open="true"]');
	const completed = pane.locator('.turn-end[data-outcome="completed"]');
	await page.getByRole("button", { name: "New session" }).click();
	await page.locator(".segment", { hasText: "Codex" }).click();
	check(
		(await page.locator(".providers .hint").innerText()) === WHAT_CODEX_DOES,
		"choosing Codex says what it does without asking",
	);
	await startSession("Codex", `${dir}/first`);
	await startSession("Codex", `${dir}/second`);
	await openRow("first");
	await send("write a.txt");
	await open.waitFor({ timeout: patience });
	check(
		(await text(".reply")) ===
			"I’ll create `a.txt` with the exact requested contents.",
		"the first reply streamed in before the request",
	);
	await openRow("second");
	await send("write b.txt");
	await open.waitFor({ timeout: patience });
	log(`second approval card: "${squeeze(await open.innerText())}"`);
	await openRow("first");
	await open.locator("button.approve").click();
	await completed.waitFor({ timeout: patience });
	check(
		(await text(".request .resolved")) === "Approved by iPhone",
		"the first request was approved by this device",
	);
	check(
		(await pane.locator(".tool").getAttribute("data-state")) === "succeeded",
		"the approved command succeeded",
	);
	check(
		(await text(".reply")).endsWith("Created [a.txt](/tmp/s2-scratch/a.txt)."),
		"the first turn ended with its own answer",
	);
	await openRow("second");
	await open.locator("button.deny").click();
	await completed.waitFor({ timeout: patience });
	check(
		(await text(".request .resolved")) === "Denied by iPhone",
		"the second request was denied by this device",
	);
	check(
		(await text(".reply")) === "The request was declined.",
		"the second turn ended with its own answer",
	);
	await page.screenshot({ path: out });
};
