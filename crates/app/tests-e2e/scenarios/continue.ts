import type { Locator } from "playwright";
import { type Run, shotNamed, tools } from "../harness.ts";

const KILLED = "The session has ended (signal: 9 (SIGKILL)).";
const REASONING =
	'"All but 9 run away" means 9 stay, so he buys double that, 18, bringing the total to 27, then sells a third (9), leaving 18 sheep.';

const options = async (select: Locator): Promise<string> =>
	(await select.locator("option").allInnerTexts()).join(", ");

// fixtures/e2e/claude-killed.jsonl, then claude-resumed.jsonl when the
// session continues. The host's model list is the same recording's.
const killedAndContinued = async (run: Run): Promise<void> => {
	const { page, out, dir, patience, log } = run;
	const { check, pane, settled, back, send, text } = tools(run);
	const composer = pane.locator(
		'.composer textarea[placeholder="Message Claude Code"]',
	);
	await back();
	await page.getByRole("button", { name: "New session" }).click();
	await page.locator(".segment", { hasText: "Claude Code" }).click();
	const model = page.locator('select[name="model"]');
	const effort = page.locator('select[name="effort"]');
	check(
		(await options(model)) === "Default (Opus), Opus, Fable, Sonnet, Haiku",
		"the model picker lists the CLI's models, the default first",
	);
	await model.selectOption({ label: "Sonnet" });
	check(
		(await options(effort)) === "Default, low, medium, high, xhigh, max",
		"the effort picker lists the model's efforts",
	);
	await effort.selectOption({ label: "high" });
	await page.screenshot({ path: shotNamed(out, "picker") });
	await page.locator('input[name="directory"]').fill(`${dir}/killed`);
	await page.getByRole("button", { name: "Start session" }).click();
	await composer.waitFor({ timeout: patience });
	check((await text(".runs-with")) === "sonnet", "the header shows the model");
	check(
		(await pane.locator("select.effort-pick").inputValue()) === "high",
		"the header shows the effort",
	);
	await send("sheep?");
	await pane
		.locator('.turn-end[data-outcome="completed"]')
		.waitFor({ timeout: patience });
	await send("count");
	const closed = pane.locator(".closed-note");
	await closed.waitFor({ timeout: patience });
	check(
		(await closed.locator("p").innerText()) === KILLED,
		"the killed CLI closes the session",
	);
	check(
		(await closed.getByRole("button", { name: "Remove" }).count()) === 1,
		"a closed session offers Remove beside Continue",
	);
	await settled();
	await page.screenshot({ path: shotNamed(out, "closed") });
	await closed.getByRole("button", { name: "Continue" }).tap();
	log("tapped Continue");
	await composer.waitFor({ timeout: patience });
	await page
		.waitForFunction(
			() => document.activeElement?.matches(".composer textarea"),
			undefined,
			{ timeout: patience },
		)
		.catch(() => undefined);
	check(
		await page.evaluate(
			() => document.activeElement?.matches(".composer textarea") ?? false,
		),
		"focus moved to the composer once the session was ready again, not the page",
	);
	check(
		(await text(".continued")) === `Continued after: ${KILLED}`,
		"the transcript keeps the close and the continue",
	);
	await send("how many sheep?");
	await pane
		.locator('.turn-end[data-outcome="completed"] >> nth=1')
		.waitFor({ timeout: patience });
	check((await text(".reply")) === "18 | 18", "the continued session answered");
	check(
		(await text(".user .bubble")) === "sheep? | count | how many sheep?",
		"every message shows once",
	);
};

// fixtures/e2e/claude-reasoning.jsonl, paced: summarized thinking. The
// CLI sends it only with betas on, which the owner's gateway refuses.
const reasoning = async (run: Run): Promise<void> => {
	const { page, out, dir, patience } = run;
	const { check, pane, startSession, send } = tools(run);
	await startSession("Claude Code", `${dir}/reasoning`);
	await send("sheep?");
	const live = pane.locator('details.reasoning[data-state="streaming"]');
	await live.waitFor({ timeout: patience });
	check(
		(await live.locator("summary").innerText()) === "Thinking…",
		'reasoning streams under "Thinking…"',
	);
	check(
		!(await live
			.locator("summary")
			.evaluate((summary) => summary.hasAttribute("role"))) &&
			(await live.locator('summary [role="status"]').count()) === 1,
		"the summary stays a disclosure and a span inside it announces the stream",
	);
	check(
		(await pane.locator(".working").count()) === 0,
		"the streaming reasoning stands in for the working line",
	);
	await pane
		.locator('.turn-end[data-outcome="completed"]')
		.waitFor({ timeout: patience });
	const done = pane.locator('details.reasoning[data-state="done"]');
	const isOpen = (): Promise<boolean> =>
		done.evaluate((details: HTMLDetailsElement) => details.open);
	check((await done.count()) === 1, "one Thinking disclosure for the turn");
	check(
		(await done.locator("summary").innerText()) === "Thinking",
		'it reads "Thinking" once finished',
	);
	check(!(await isOpen()), "it starts collapsed");
	await done.locator("summary").tap();
	check(await isOpen(), "a tap expands it");
	check(
		(await done.locator(".reasoning-text").innerText()).trim() === REASONING,
		"it shows the reasoning text",
	);
	await page.screenshot({ path: out });
};

export const continueSession = async (run: Run): Promise<void> => {
	await killedAndContinued(run);
	await reasoning(run);
};
