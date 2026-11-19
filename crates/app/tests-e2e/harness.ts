import type { Browser, Locator, Page } from "playwright";

const MS_PER_SECOND = 1000;
const STAMP_DECIMALS = 3;
const STAMP_WIDTH = 7;
const SPACES = /\s+/gu;
const PNG = /\.png$/u;

interface SavedHost {
	readonly ticket?: string;
	readonly name?: string;
}

const onHost = (host: string | undefined): string => {
	if (host === undefined) {
		return "";
	}
	return ` on ${host}`;
};

/** Waits until the page has painted and its finite animations and
 * transitions have ended. */
const settle = (page: Page): Promise<void> =>
	page.evaluate(async () => {
		const frame = (): Promise<number> =>
			new Promise((painted) => {
				requestAnimationFrame(painted);
			});
		await frame();
		await frame();
		await Promise.all(
			document
				.getAnimations()
				.filter(
					(a) =>
						a.effect?.getComputedTiming().endTime !== Number.POSITIVE_INFINITY,
				)
				.map((a) => a.finished.catch(() => undefined)),
		);
	});

const backOn = async (page: Page): Promise<void> => {
	const button = page.locator(".pane-head .back");
	if (await button.isVisible()) {
		await button.click();
	}
};

/** The smallest target a thumb can hit, in CSS pixels. */
export const THUMB = 44;
export const PHONE = { width: 390, height: 844 } as const;

// fixtures/e2e/claude-approve.jsonl: one Write, approved, then the reply.
export const PROMPT = "make a file";

/** What a scenario is given: the page it drives and the run's effects. */
export interface Run {
	readonly browser: Browser;
	readonly page: Page;
	readonly joinUrl: string;
	readonly out: string;
	readonly dir: string;
	readonly secondJoin: string | undefined;
	readonly patience: number;
	readonly now: () => number;
	readonly log: (line: string) => void;
	readonly watch: (page: Page) => void;
	readonly consoleError: (text: string) => void;
}

/** The steps and checks every scenario drives the app's page with. */
export interface Tools {
	readonly check: (isMet: boolean, what: string) => void;
	readonly status: Locator;
	readonly statusLine: () => Promise<string>;
	readonly waitFor: (selector: string) => Promise<void>;
	/** Waits until the page has painted and its finite animations and
	 * transitions have ended, so a screenshot shows where they settle. */
	readonly settled: (p?: Page) => Promise<void>;
	readonly pane: Locator;
	/** Back to the session list; only a phone has the button. */
	readonly back: (p?: Page) => Promise<void>;
	readonly openRow: (title: string, p?: Page) => Promise<void>;
	/** Starts a session from the New session form and waits until it takes a
	 * message or has closed. */
	readonly startSession: (
		agent: string,
		directory: string,
		host?: string,
	) => Promise<void>;
	readonly send: (prompt: string) => Promise<void>;
	readonly text: (selector: string) => Promise<string>;
}

export const stamp = (ms: number, line: string): string =>
	`[${(ms / MS_PER_SECOND).toFixed(STAMP_DECIMALS).padStart(STAMP_WIDTH)}s] ${line}`;

export const squeeze = (text: string): string => text.replace(SPACES, " ");

export const shotNamed = (out: string, suffix: string): string =>
	out.replace(PNG, `-${suffix}.png`);

export const lastSegment = (path: string): string =>
	path.split("/").at(-1) ?? "";

export const savedHosts = async (page: Page): Promise<readonly SavedHost[]> => {
	const stored = await page.evaluate(() =>
		localStorage.getItem("conductor.hosts"),
	);
	const hosts: Readonly<Record<string, SavedHost>> = JSON.parse(String(stored));
	return Object.values(hosts);
};

/** Everything the page keeps or shows, to look for a secret in. */
export const pageKeeps = (page: Page): Promise<string> =>
	page.evaluate(async () =>
		JSON.stringify([
			{ ...localStorage },
			{ ...sessionStorage },
			location.href,
			document.documentElement.outerHTML,
			await indexedDB.databases(),
		]),
	);

export const isWithinWidth = (page: Page): Promise<boolean> =>
	page.evaluate(
		() => document.documentElement.scrollWidth <= globalThis.innerWidth,
	);

export const boxOf = async (
	locator: Locator,
): Promise<{ readonly width: number; readonly height: number }> => {
	const box = await locator.boundingBox();
	if (box === null) {
		throw new Error(`no box for ${locator.toString()}`);
	}
	return box;
};

export const tools = (run: Run): Tools => {
	const { page, patience, log } = run;
	const status = page.locator("header.status");
	const pane = page.locator(".session-pane");
	const back = (p: Page = page): Promise<void> => backOn(p);
	return {
		check: (isMet: boolean, what: string): void => {
			if (!isMet) {
				throw new Error(`FAILED: ${what}`);
			}
			log(`ok: ${what}`);
		},
		status,
		statusLine: async (): Promise<string> => squeeze(await status.innerText()),
		waitFor: (selector: string): Promise<void> =>
			page.locator(selector).waitFor(),
		settled: (p: Page = page): Promise<void> => settle(p),
		pane,
		back,
		openRow: async (title: string, p: Page = page): Promise<void> => {
			await back(p);
			await p
				.locator(".session-row", {
					has: p.locator(".row-title", {
						hasText: new RegExp(`^${title}$`, "u"),
					}),
				})
				.click();
			await p.locator(".session-pane").waitFor({ timeout: patience });
		},
		startSession: async (
			agent: string,
			directory: string,
			host?: string,
		): Promise<void> => {
			await back();
			await page.getByRole("button", { name: "New session" }).click();
			if (host !== undefined) {
				await page.locator('select[name="host"]').selectOption({ label: host });
			}
			await page.locator(".segment", { hasText: agent }).click();
			await page.locator('input[name="directory"]').fill(directory);
			await page.getByRole("button", { name: "Start session" }).click();
			await pane
				.locator(
					`.composer textarea[placeholder="Message ${agent}"], .closed-note`,
				)
				.waitFor({ timeout: patience });
			log(`started a ${agent} session in ${directory}${onHost(host)}`);
		},
		send: async (prompt: string): Promise<void> => {
			const composer = pane.locator(".composer textarea");
			await composer.fill(prompt);
			await composer.press("Enter");
			log(`sent "${prompt}"`);
		},
		text: async (selector: string): Promise<string> =>
			(await pane.locator(selector).allInnerTexts()).join(" | "),
	};
};
