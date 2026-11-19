// Drives the web build at an iPhone viewport against a live `conductord host`
// on the fake CLIs, through n0's relays. ENGINE picks
// chromium or webkit; webkit stands in for iOS Safari, which Chromium does
// not catch (it accepted the relay's root-dot hostname that Safari rejected).
//
// usage: ENGINE=webkit node browser.ts <join-url> <out> <scenario> <dir>
//
// Scenarios, each against its own host and each in scenarios/. <dir> is the
// session directory on the host, holding the fake's recording; <out> is a
// screenshot, or for screens the directory the screenshots go to.
//   turn      Live on the relay, a Claude turn through approval, reload,
//             and pairing a fresh install.
//   worktrees A session's checkpointed turns: a turn's changes shown and
//             one undone with every later turn, across a reload.
//   codex     Two Codex sessions in <dir>/first and <dir>/second, one
//             approved and one denied.
//   credentials  The forms the host's catalog describes, and a Claude
//             gateway entered in its form, which makes the instance of the
//             gateway's account beside Claude's own; that instance removed,
//             added again and listed in the new-session picker; <dir> is
//             unused.
//   sign-in   Claude signed in from the app on the fake's sign-in: the
//             link it printed, a refused code, then the accepted one, and
//             the account the CLI named shown as an instance, signed in,
//             without a reload; <dir> is unused, and <out> also gets a
//             -refused shot.
//   questions A Claude question in <dir>/question answered by tapping an
//             option, and a command in <dir>/for-session allowed for the
//             session, which then runs again without asking.
//   continue  A Claude session in <dir>/killed started with a model and
//             an effort picked in the form, its CLI killed mid-turn and
//             continued; and one in <dir>/reasoning whose reasoning streams
//             under "Thinking…" and folds into a "Thinking" disclosure.
//   tasks     A Claude session in <dir> whose turn keeps a checklist, shown
//             as its tasks, and whose effort is changed in the header
//             before the next turn; <out> is the phone screenshot.
//   subagents A sub-agent folded under the call that started it, opened,
//             and kept across a reload.
//   screens   Sessions in <dir>/conductor (held mid-reply), <dir>/scratch
//             (the CLI hangs up) and <dir>/notes (never sent to), shot at
//             1280x800 and 390x844, light and dark.
//   hosts     Two hosts, desk (the join link) and laptop (SECOND_JOIN),
//             paired in one app: a Claude session started on each from the
//             one form, each listed under its own host, both kept across a
//             reload, desk paired again, and laptop forgotten.
//   projects  A host whose home lists alpha, checked out in <dir>/roots,
//             and beta, cloned from the projects view; then a Claude
//             session started in alpha from the form's project picker.
//
// This file is the boundary: it alone reads the arguments, the environment
// and the clock, and writes to the console.

import { type Browser, chromium, type Page, webkit } from "playwright";
import {
	JOIN_FRAGMENT,
	LIVE,
	MS_PER_SECOND,
	PHONE,
	type Run,
	squeeze,
	stamp,
	tools,
} from "./harness.ts";
import { checkConsole, type Recorder, recorder } from "./records.ts";
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

// Every wait is on something the page or the host shows, so none needs a
// tight bound: each may take up to PATIENCE, which only a broken step uses
// up, and the whole scenario up to DEADLINE.
const {
	E2E_PATIENCE: patienceSetting,
	E2E_DEADLINE: deadlineSetting,
	ENGINE: engine = "chromium",
	SECOND_JOIN: secondJoin,
	E2E_FAILURES: failures,
}: Readonly<Record<string, string | undefined>> = process.env;
const DEFAULT_PATIENCE_MS = 120_000;
const DEFAULT_DEADLINE_MS = 600_000;
const PATIENCE = Number(patienceSetting ?? DEFAULT_PATIENCE_MS);
const DEADLINE = Number(deadlineSetting ?? DEFAULT_DEADLINE_MS);
const FAILURE_SHOT_MS = 10_000;
const USAGE_EXIT = 2;
const FIRST_ARGUMENT = 2;

// An iPhone's user agent, so the app names itself "iPhone" in Hello.
const IPHONE_UA =
	"Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Mobile/15E148 Safari/604.1";

/** The run's positional arguments: usage: node browser.ts <join-url> <out>
 * <scenario> <dir>. */
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

const traceOf = (error: unknown): string => {
	if (error instanceof Error && error.stack !== undefined) {
		return error.stack;
	}
	return String(error);
};

const messageOf = (error: unknown): string => {
	if (error instanceof Error) {
		return error.message;
	}
	return String(error);
};

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

const launch = async (
	scenario: string,
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
	context.setDefaultTimeout(PATIENCE);
	return { browser, page: await context.newPage() };
};

/** The page as it was at a failure, and its status line, so a failure under
 * load says what the page was waiting on. */
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
				timeout: FAILURE_SHOT_MS,
			});
			log(`failure screenshot ${shot}`);
		}
	} catch (error) {
		log(`no failure screenshot: ${messageOf(error)}`);
	}
};

// A failed step leaves the page as it was and what the app last logged.
const onFailure =
	({ run, record, scenario }: Started) =>
	async (failure: unknown): Promise<void> => {
		run.log(traceOf(failure));
		await shootFailure(run, scenario);
		run.log("last console lines:");
		for (const line of record.tail()) {
			console.log(`    ${line}`);
		}
		process.exit(1);
	};

const clock = (): {
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
			console.log(stamp(elapsed(), line));
		},
	};
};

const armDeadline = (): void => {
	setTimeout(() => {
		throw new Error(
			`FAILED: the scenario took longer than ${DEADLINE / MS_PER_SECOND}s`,
		);
	}, DEADLINE).unref();
};

const start = async (args: readonly string[]): Promise<Started> => {
	const { joinUrl, out, scenario, dir } = argumentsOf(args);
	const { now, elapsed, log } = clock();
	armDeadline();
	const { browser, page } = await launch(scenario, log);
	const record = recorder(elapsed);
	record.watch(page);
	const run: Run = {
		browser,
		page,
		joinUrl,
		out,
		dir,
		secondJoin,
		patience: PATIENCE,
		now,
		log,
		watch: record.watch,
		consoleError: record.consoleError,
	};
	const started = { run, record, scenario };
	process.on("uncaughtException", onFailure(started));
	return started;
};

const connect = async (run: Run): Promise<void> => {
	const { page, joinUrl, log, now } = run;
	const { statusLine, waitFor } = tools(run);
	log(`open ${joinUrl.replace(JOIN_FRAGMENT, "#join=<ticket>")}`);
	const opened = now();
	await page.goto(joinUrl);
	await waitFor(LIVE);
	log(`Live ${now() - opened}ms after navigation (first connect)`);
	log(`status line: "${await statusLine()}"`);
};

const main = async (args: readonly string[]): Promise<void> => {
	const { run, record, scenario } = await start(args);
	await connect(run);
	if (!isScenario(scenario)) {
		throw new Error(`unknown scenario ${scenario}`);
	}
	await SCENARIOS[scenario](run);
	checkConsole(run, record);
	await run.browser.close();
	run.log("PASS");
};

await main(process.argv.slice(FIRST_ARGUMENT));
