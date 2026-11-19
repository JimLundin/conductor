import type { BrowserContext, Locator, Page } from "playwright";
import { LIVE, PHONE, PROMPT, type Run, tools } from "../harness.ts";

type State = "running" | "closed" | "idle";

interface Shooter {
	readonly scheme: "light" | "dark";
	readonly device: string;
	readonly states: readonly (readonly [string, State])[];
	readonly context: BrowserContext;
	readonly view: Page;
}

const SCHEMES = ["light", "dark"] as const;
const SHOTS = [
	{
		device: "desktop",
		options: { viewport: { width: 1280, height: 800 } },
		states: [
			["conductor", "running"],
			["scratch", "closed"],
		],
	},
	{
		device: "phone",
		options: {
			viewport: PHONE,
			deviceScaleFactor: 2,
			isMobile: true,
			hasTouch: true,
		},
		states: [
			["conductor", "running"],
			["notes", "idle"],
		],
	},
] as const;

/** A page per scheme and device, each Live. Deltas are live-only, so every
 * shooter is Live before the reply starts. */
const shooters = async (run: Run): Promise<readonly Shooter[]> => {
	const { browser, joinUrl, patience, watch } = run;
	let ready: readonly Shooter[] = [];
	for (const scheme of SCHEMES) {
		for (const { device, options, states } of SHOTS) {
			const context = await browser.newContext({
				...options,
				colorScheme: scheme,
				reducedMotion: "reduce",
			});
			context.setDefaultTimeout(patience);
			const view = await context.newPage();
			watch(view);
			await view.goto(joinUrl);
			await view.locator(LIVE).waitFor({ timeout: patience });
			ready = [...ready, { scheme, device, states, context, view }];
		}
	}
	return ready;
};

const shown = (view: Page, state: State): Locator =>
	({
		running: view.locator(".reply.streaming", { hasText: "/tmp/s1" }),
		closed: view.locator(".session-pane .closed-note"),
		idle: view.locator(".session-pane .composer textarea:not([disabled])"),
	})[state];

export const screens = async (run: Run): Promise<void> => {
	const { out, dir, patience, log } = run;
	const { pane, settled, openRow, startSession, send, text } = tools(run);
	await startSession("Claude Code", `${dir}/notes`);
	await startSession("Claude Code", `${dir}/scratch`);
	await pane.locator(".closed-note").waitFor({ timeout: patience });
	log(`closed: "${await text(".closed-note")}"`);
	await startSession("Claude Code", `${dir}/conductor`);
	await send(PROMPT);
	const approve = pane.locator('.request[data-open="true"] button.approve');
	await approve.waitFor({ timeout: patience });
	const ready = await shooters(run);
	await approve.click();
	await pane
		.locator(".reply.streaming", { hasText: "/tmp/s1" })
		.waitFor({ timeout: patience });
	log(`running, held mid-reply: "${await text(".reply")}"`);
	for (const { scheme, device, states, context, view } of ready) {
		for (const [title, state] of states) {
			await openRow(title, view);
			await shown(view, state).waitFor();
			await settled(view);
			const file = `${out}/${device}-${scheme}-${state}.png`;
			await view.screenshot({ path: file });
			log(`screenshot ${file}`);
		}
		await context.close();
	}
};
