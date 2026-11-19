import {
	boxOf,
	COMPLETED,
	type Run,
	shotNamed,
	squeeze,
	THUMB,
	tools,
} from "../harness.ts";

const OPTION_WIDTH = 300;
const QUESTION_TITLE = /^question$/u;

const activeId = (run: Run): Promise<string | undefined> =>
	run.page.evaluate(() => document.activeElement?.id);

// fixtures/e2e/claude-question.jsonl: Claude asks which color, the answer
// goes back, and it replies with it.
const question = async (run: Run): Promise<void> => {
	const { page, out, dir, patience, log } = run;
	const { check, pane, settled, back, openRow, startSession, send, text } =
		tools(run);
	await startSession("Claude Code", `${dir}/question`);
	await send("ask me");
	const card = pane.locator('.question[data-open="true"]');
	await card.waitFor({ timeout: patience });
	log(`question card: "${squeeze(await card.innerText())}"`);
	const sendAnswer = card.getByRole("button", { name: "Send answer" });
	check(
		await sendAnswer.isDisabled(),
		"the answer cannot be sent before an option is chosen",
	);
	await back();
	const row = page.locator(".session-row", {
		has: page.locator(".row-title", { hasText: QUESTION_TITLE }),
	});
	check(
		(await row.locator(".row-sub").innerText()) === "Claude Code, needs you",
		"the session list says the question waits on you",
	);
	check(
		(await row.locator(".badge").innerText()) === "1",
		"the badge counts the open question",
	);
	await openRow("question");
	const blue = card.locator("button.option", { hasText: "Blue" });
	await blue.tap();
	check(
		(await blue.getAttribute("aria-pressed")) === "true",
		"the tapped option shows as chosen",
	);
	const box = await boxOf(blue);
	check(
		box.height >= THUMB && box.width >= OPTION_WIDTH,
		`the option is a thumb-sized target (${Math.round(box.width)}x${Math.round(box.height)})`,
	);
	await settled();
	await page.screenshot({ path: shotNamed(out, "question") });
	await sendAnswer.tap();
	log("tapped Send answer");
	await pane.locator(COMPLETED).waitFor({ timeout: patience });
	check(
		(await text(".question .question-answer")) === "Blue",
		"the card shows the answer given",
	);
	check(
		(await text(".question .resolved")) === "Answered by iPhone",
		"the question was answered once, by this device",
	);
	check(
		(await activeId(run)) === "request-1",
		"focus moved to the answered card, not the page",
	);
	check((await text(".reply")) === "Blue", "Claude replied with the answer");
	check(
		(await pane.locator(".tool").count()) === 0,
		"the question shows no tool card of its own",
	);
};

// fixtures/e2e/claude-for-session.jsonl: `touch t1.txt` asks, is allowed
// for the session, and runs again in the next turn without asking.
const forSession = async (run: Run): Promise<void> => {
	const { page, out, dir, patience, log } = run;
	const { check, pane, settled, startSession, send, text } = tools(run);
	await startSession("Claude Code", `${dir}/for-session`);
	await send("touch t1.txt");
	const request = pane.locator('.request[data-open="true"]');
	await request.waitFor({ timeout: patience });
	const allow = request.locator("button.for-session");
	check(
		squeeze(await allow.innerText()) ===
			"Allow for this session Allows Bash(touch t1.txt) until the session ends",
		"the third button says what it allows, in the CLI's words",
	);
	await settled();
	await page.screenshot({ path: shotNamed(out, "for-session") });
	await allow.tap();
	log("tapped Allow for this session");
	await pane.locator(COMPLETED).waitFor({ timeout: patience });
	check(
		(await text(".request .resolved")) === "Allowed for this session by iPhone",
		"the request was allowed for the session by this device",
	);
	check(
		(await activeId(run)) === "request-1",
		"focus moved to the resolved approval card, not the page",
	);
	await send("again");
	await pane.locator(`${COMPLETED} >> nth=1`).waitFor({ timeout: patience });
	check(
		(await pane.locator(".request").count()) === 1,
		"the second identical call asked nothing",
	);
	check(
		(
			await pane
				.locator(".tool")
				.evaluateAll((cards) =>
					cards.map((card) => card.getAttribute("data-state")),
				)
		).join(" ") === "succeeded succeeded",
		"both calls ran",
	);
	await page.screenshot({ path: out });
};

export const questions = async (run: Run): Promise<void> => {
	await question(run);
	await forSession(run);
};
