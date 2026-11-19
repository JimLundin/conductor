import {
	boxOf,
	COMPLETED,
	JOIN_FRAGMENT,
	lastSegment,
	LIVE,
	PHONE,
	PROMPT,
	type Run,
	savedHosts,
	squeeze,
	THUMB,
	tools,
} from "../harness.ts";

const WRITTEN = "/tmp/s1-scratch/a.txt";
const REPLY =
	"I created `a.txt` in `/tmp/s1-scratch` with the content `hello from s1`.";
const LIVE_PARTIALS = 3;
const BEFORE_TICKET = /.*#join=/u;
const CONTROLS = [
	".request button.approve",
	".request button.deny",
	".composer textarea",
	".composer button",
] as const;

/** A Claude session in `dir`, one message through approval to the end of the
 * turn. The recording asks before it replies, so the reply streams after
 * Approve. Each control is measured once the approval card shows. Checks the
 * whole turn rendered exactly once. */
const approvedTurn = async (run: Run): Promise<void> => {
	const { page, dir, patience, log } = run;
	const { check, pane, startSession, send, text } = tools(run);
	await startSession("Claude Code", dir);
	await send(PROMPT);
	const approve = pane.locator('.request[data-open="true"] button.approve');
	await approve.waitFor({ timeout: patience });
	log(
		`approval card: "${squeeze(await pane.locator('.request[data-open="true"]').innerText())}"`,
	);
	for (const control of CONTROLS) {
		const box = await boxOf(pane.locator(control).last());
		check(
			box.height >= THUMB && box.width >= THUMB,
			`${control} is at least ${THUMB}px (${box.width.toFixed(0)}x${box.height.toFixed(0)})`,
		);
	}
	// Each text the streaming reply shows, recorded in the page as it renders,
	// so a busy machine can't step over a delta between two reads.
	await page.evaluate(() => {
		const streamed = new Set<string>();
		Object.assign(globalThis, { streamed });
		new MutationObserver(() => {
			const reply = document.querySelector<HTMLElement>(
				".session-pane .reply.streaming",
			);
			if (reply !== null) {
				streamed.add(reply.innerText);
			}
		}).observe(document.body, {
			subtree: true,
			childList: true,
			characterData: true,
		});
	});
	await approve.tap();
	log("tapped Approve");
	await pane.locator(COMPLETED).waitFor({ timeout: patience });
	const seen = await page.evaluate(() => {
		const streamed: ReadonlySet<string> = Reflect.get(globalThis, "streamed");
		return streamed.size;
	});
	check(
		seen >= LIVE_PARTIALS,
		`deltas rendered live (${seen} distinct partial texts seen)`,
	);
	check((await text(".user .bubble")) === PROMPT, "the message shows once");
	check(
		(await text(".tool .tool-detail")) === WRITTEN,
		"one tool card, for the written file",
	);
	check(
		(await text(".tool .tool-name")) === "Write",
		"the card names the change a Write",
	);
	check(
		(await pane.locator(".tool").getAttribute("data-state")) === "succeeded",
		"the tool succeeded",
	);
	check(
		(await text(".request .resolved")) === "Approved by iPhone",
		"the approval resolved once, by this device",
	);
	check(
		(await text(".reply")) === REPLY,
		"the streamed reply is every delta exactly once",
	);
	check(
		(await text(".turn-end")) === "Turn finished",
		'exactly one "Turn finished"',
	);
};

/** The composer's inset with the keyboard down and up. Playwright can neither
 * open the keyboard nor give the page a home indicator, so the inset is set
 * by hand; a reload drops it. */
const composerInset = async (run: Run): Promise<void> => {
	const { page } = run;
	const { check, pane } = tools(run);
	await page.addStyleTag({ content: ".composer { --inset-bottom: 34px; }" });
	const padding = (): Promise<string> =>
		pane
			.locator(".composer")
			.evaluate((el) => getComputedStyle(el).paddingBottom);
	check(
		(await padding()) === "42px",
		"the composer keeps the home indicator inset with the keyboard down",
	);
	await pane.locator(".composer textarea").tap();
	check(
		(await padding()) === "8px",
		"the composer drops the inset while it has focus, as with the keyboard up",
	);
};

/** A Home Screen web app on iOS starts with empty storage and no fragment, so
 * it must pair by pasting the join link. The camera can't hand a new join
 * link to a Home Screen app, so a paired app must be able to drop its host and
 * pair again from inside. */
const freshInstall = async (run: Run): Promise<void> => {
	const { browser, joinUrl, patience, log, now } = run;
	const { check } = tools(run);
	const fresh = await browser.newContext({
		viewport: PHONE,
		isMobile: true,
		hasTouch: true,
	});
	fresh.setDefaultTimeout(patience);
	const installed = await fresh.newPage();
	installed.on("console", (msg) => {
		if (msg.type() === "error") {
			run.consoleError(msg.text());
		}
	});
	const asksToPair = installed.getByRole("heading", {
		name: "Connect to a host",
	});
	const live = installed.locator(LIVE);
	await installed.goto(joinUrl.replace(JOIN_FRAGMENT, ""));
	await asksToPair.waitFor({ timeout: patience });
	check(true, "a fresh install asks for a join link");
	await installed.locator("form.pair input").fill(joinUrl);
	const pasted = now();
	await installed.locator("form.pair button").click();
	await live.waitFor({ timeout: patience });
	log(`Live ${now() - pasted}ms after pasting the join link`);
	await installed.getByRole("button", { name: "Credentials" }).click();
	await installed.getByRole("button", { name: "Forget conductor" }).click();
	await asksToPair.waitFor({ timeout: patience });
	await installed.reload();
	await asksToPair.waitFor({ timeout: patience });
	check(
		true,
		"forgetting the only host asks for a join link, across a relaunch",
	);
	await installed.locator("form.pair input").fill(joinUrl);
	await installed.locator("form.pair button").click();
	await live.waitFor({ timeout: patience });
	check(true, "pairing again after forgetting the host reaches Live");
	await fresh.close();
};

export const turn = async (run: Run): Promise<void> => {
	const { page, joinUrl, out, dir, patience, log, now } = run;
	const { check, pane, openRow, waitFor, text } = tools(run);
	check(
		!page.url().includes("#join="),
		"the join fragment was removed from the address bar",
	);
	const saved = await savedHosts(page);
	check(
		saved.length === 1 &&
			saved[0]?.ticket === joinUrl.replace(BEFORE_TICKET, ""),
		"the host and its ticket are in local storage, keyed by its id",
	);
	// This host runs no Codex CLI.
	await page.getByRole("button", { name: "New session" }).click();
	await page.locator(".segment", { hasText: "Codex" }).click();
	const why = await page.locator(".providers .hint").innerText();
	check(
		why.startsWith("Codex can't run on conductor: "),
		`the form says why Codex is unavailable on the host, by name ("${why}")`,
	);

	await approvedTurn(run);
	await page.screenshot({ path: out });
	log(`screenshot ${out}`);
	await composerInset(run);

	const reloaded = now();
	await page.goto(joinUrl.replace(JOIN_FRAGMENT, ""));
	await waitFor(LIVE);
	log(
		`Live ${now() - reloaded}ms after reload without the fragment (reconnect)`,
	);
	await openRow(lastSegment(dir));
	await pane.locator(".turn-end").waitFor({ timeout: patience });
	check(
		(await text(".reply")) === REPLY,
		"the transcript replayed after reload",
	);

	await freshInstall(run);
};
