// Usage: ENGINE=webkit node browser.ts <join-url> <out> <scenario> <dir>
// <dir> holds the host's fake CLI recording. The screens scenario writes to
// a directory at <out>; other scenarios write a screenshot file.

import { type Browser, chromium, type Page, webkit } from "playwright";
import {
	LIVE,
	MS_PER_SECOND,
	PHONE,
	type Run,
	squeeze,
	stamp,
	tools,
} from "./harness.ts";
import {
	checkConsole,
	messageOf,
	pairingRedactor,
	type Recorder,
	recorder,
	traceOf,
} from "./records.ts";
import { codex } from "./scenarios/codex.ts";
import { continueSession } from "./scenarios/continue.ts";
import { credentials } from "./scenarios/credentials.ts";
import { hosts } from "./scenarios/hosts.ts";
import { projects } from "./scenarios/projects.ts";
import { questions } from "./scenarios/questions.ts";
import { screens } from "./scenarios/screens.ts";
import { signIn } from "./scenarios/sign-in.ts";
import { subagents } from "./scenarios/subagents.ts";
import { tasks } from "./scenarios/tasks.ts";
import { turn } from "./scenarios/turn.ts";
import { worktrees } from "./scenarios/worktrees.ts";

const SCENARIOS = {
	turn,
	worktrees,
	codex,
	questions,
	continue: continueSession,
	tasks,
	subagents,
	credentials,
	"sign-in": signIn,
	screens,
	hosts,
	projects,
} as const;
const ENGINES = { chromium, webkit } as const;

const {
	E2E_PATIENCE: patienceSetting,
	E2E_DEADLINE: deadlineSetting,
	ENGINE: engine = "chromium",
	SECOND_JOIN: secondJoin,
	E2E_FAILURES: failures,
}: Readonly<Record<string, string | undefined>> = process.env;
const DEFAULT_PATIENCE_MS = 120_000;
const DEFAULT_DEADLINE_MS = 600_000;
const MAX_TIMEOUT_MS = 2_147_483_647;
const FAILURE_DIAGNOSTICS_MS = 10_000;
const USAGE_EXIT = 2;
const FIRST_ARGUMENT = 2;

// An iPhone's user agent, so the app names itself "iPhone" in Hello.
const IPHONE_UA =
	"Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Mobile/15E148 Safari/604.1";

interface Arguments {
	readonly joinUrl: string;
	readonly out: string;
	readonly scenario: string;
	readonly dir: string;
}

interface Started {
	readonly run: Run;
	readonly record: Recorder;
	readonly scenario: string;
}

const isScenario = (name: string): name is keyof typeof SCENARIOS =>
	Object.hasOwn(SCENARIOS, name);
const isEngine = (name: string): name is keyof typeof ENGINES =>
	Object.hasOwn(ENGINES, name);

const argumentsOf = (args: readonly string[]): Arguments => {
	const [joinUrl, out, scenario, dir] = args;
	if (
		joinUrl === undefined ||
		out === undefined ||
		scenario === undefined ||
		!dir
	) {
		console.error(
			`usage: node browser.ts <join-url> <out> <${Object.keys(SCENARIOS).join("|")}> <dir>`,
		);
		process.exit(USAGE_EXIT);
	}
	return { joinUrl, out, scenario, dir };
};

const duration = (
	name: string,
	setting: string | undefined,
	fallback: number,
): number => {
	const ms = Number(setting ?? fallback);
	if (!Number.isInteger(ms) || ms <= 0 || ms > MAX_TIMEOUT_MS) {
		throw new RangeError(
			`${name} must be an integer between 1 and ${MAX_TIMEOUT_MS} milliseconds`,
		);
	}
	return ms;
};

const launch = async (
	scenario: string,
	patience: number,
	log: (line: string) => void,
): Promise<{ readonly browser: Browser; readonly page: Page }> => {
	if (!isEngine(engine)) {
		throw new TypeError(`unknown engine ${engine}`);
	}
	const browser = await ENGINES[engine].launch();
	log(`engine: ${engine} ${browser.version()}, scenario: ${scenario}`);
	const context = await browser.newContext({
		viewport: PHONE,
		deviceScaleFactor: 3,
		isMobile: true,
		hasTouch: true,
		userAgent: IPHONE_UA,
	});
	context.setDefaultTimeout(patience);
	return { browser, page: await context.newPage() };
};

const shootFailure = async (run: Run, scenario: string): Promise<void> => {
	const { page, log } = run;
	try {
		log(
			`status line at failure: "${squeeze((await page.locator("header.status").allInnerTexts()).join(" | "))}"`,
		);
		if (failures) {
			const shot = `${failures}/${scenario}-${engine}.png`;
			await page.screenshot({
				path: shot,
				fullPage: true,
				timeout: FAILURE_DIAGNOSTICS_MS,
			});
			log(`failure screenshot ${shot}`);
		}
	} catch (error) {
		log(`no failure screenshot: ${messageOf(error)}`);
	}
};

const reportFailure = async ({
	run,
	record,
	scenario,
}: Started): Promise<void> => {
	run.log("last console lines:");
	for (const line of record.tail()) {
		run.log(`    ${line}`);
	}
	await shootFailure(run, scenario);
};

const failureHandler = (
	log: Run["log"],
): ((failure: unknown, started?: Started) => void) => {
	let hasFailed = false;
	return (failure: unknown, started?: Started): void => {
		if (hasFailed) {
			return;
		}
		hasFailed = true;
		setTimeout(() => process.exit(1), FAILURE_DIAGNOSTICS_MS);
		log(traceOf(failure));
		if (started === undefined) {
			process.exit(1);
		}
		reportFailure(started)
			.catch((error: unknown) => log(traceOf(error)))
			.finally(() => process.exit(1));
	};
};

const clock = (
	redact: (line: string) => string,
): {
	readonly now: () => number;
	readonly elapsed: () => number;
	readonly log: (line: string) => void;
} => {
	const t0 = Date.now();
	const now = (): number => Date.now();
	const elapsed = (): number => now() - t0;
	return {
		now,
		elapsed,
		log: (line: string): void => {
			console.log(stamp(elapsed(), redact(line)));
		},
	};
};

const start = async (
	{ joinUrl, out, scenario, dir }: Arguments,
	patience: number,
	{ now, elapsed, log }: ReturnType<typeof clock>,
): Promise<Started> => {
	const { browser, page } = await launch(scenario, patience, log);
	const record = recorder(elapsed);
	record.watch(page);
	const run: Run = {
		browser,
		page,
		joinUrl,
		out,
		dir,
		secondJoin,
		patience,
		now,
		log,
		watch: record.watch,
		consoleError: record.consoleError,
	};
	return { run, record, scenario };
};

const connect = async (run: Run): Promise<void> => {
	const { page, joinUrl, log, now } = run;
	const { statusLine, waitFor } = tools(run);
	log(`open ${joinUrl}`);
	const opened = now();
	await page.goto(joinUrl);
	await waitFor(LIVE);
	log(`Live ${now() - opened}ms after navigation (first connect)`);
	log(`status line: "${await statusLine()}"`);
};

const drive = async ({ run, record, scenario }: Started): Promise<void> => {
	await connect(run);
	if (!isScenario(scenario)) {
		throw new Error(`unknown scenario ${scenario}`);
	}
	await SCENARIOS[scenario](run);
	checkConsole(run, record);
	await run.browser.close();
};

const beforeDeadline = async (
	deadline: number,
	run: () => Promise<void>,
): Promise<void> => {
	const { promise, reject } = Promise.withResolvers<void>();
	const timer = setTimeout(() => {
		reject(
			new Error(
				`FAILED: the scenario took longer than ${deadline / MS_PER_SECOND}s`,
			),
		);
	}, deadline);
	try {
		await Promise.race([run(), promise]);
	} finally {
		clearTimeout(timer);
	}
};

const main = async (args: readonly string[]): Promise<void> => {
	const parsed = argumentsOf(args);
	const time = clock(pairingRedactor([parsed.joinUrl, secondJoin]));
	const fail = failureHandler(time.log);
	let started: Started | undefined;
	const onFailure = (failure: unknown): void => fail(failure, started);
	const runScenario = async (): Promise<void> => {
		const patience = duration(
			"E2E_PATIENCE",
			patienceSetting,
			DEFAULT_PATIENCE_MS,
		);
		const deadline = duration(
			"E2E_DEADLINE",
			deadlineSetting,
			DEFAULT_DEADLINE_MS,
		);
		await beforeDeadline(deadline, async () => {
			started = await start(parsed, patience, time);
			await drive(started);
		});
		time.log("PASS");
	};
	await runScenario().catch(onFailure);
};

await main(process.argv.slice(FIRST_ARGUMENT));
