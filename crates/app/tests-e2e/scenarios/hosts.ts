import type { Locator } from "playwright";
import { LIVE, type Run, savedHosts, tools } from "../harness.ts";

const HOSTS = ["desk", "laptop"] as const;

/** The session list, grouped by host. */
interface HostList {
	readonly group: (name: string) => Locator;
	readonly rowsOf: (name: string) => Promise<string>;
	readonly names: () => Promise<string>;
	readonly bothLive: () => Promise<void>;
	/** Pairs a host from its join link, and waits until both are Live. */
	readonly pair: (join: string) => Promise<void>;
}

const listOf = (run: Run): HostList => {
	const { page, patience } = run;
	const group = (name: string): Locator =>
		page.locator(".host-group", {
			has: page.locator(".status .host", {
				hasText: new RegExp(`^${name}$`, "u"),
			}),
		});
	const rowsOf = async (name: string): Promise<string> =>
		(await group(name).locator(".row-title").allInnerTexts()).join(", ");
	const names = async (): Promise<string> =>
		(await page.locator(".host-group .status .host").allInnerTexts()).join(
			", ",
		);
	const bothLive = async (): Promise<void> => {
		for (const name of HOSTS) {
			await group(name).locator(LIVE).waitFor({ timeout: patience });
		}
	};
	const pair = async (join: string): Promise<void> => {
		await page.getByRole("button", { name: "Add host" }).click();
		await page.locator(".detail form.pair input").fill(join);
		await page.locator(".detail form.pair button").click();
		await bothLive();
	};
	return { group, rowsOf, names, bothLive, pair };
};

/** laptop paired beside desk, and a session started on each. */
const pairedAndStarted = async (run: Run): Promise<void> => {
	const { page, dir, secondJoin, log, out } = run;
	const { check, pane, back, startSession } = tools(run);
	const { group, rowsOf, names, pair } = listOf(run);
	if (secondJoin === undefined) {
		throw new Error("FAILED: the hosts scenario needs SECOND_JOIN");
	}
	await pair(secondJoin);
	check(
		(await names()) === "desk, laptop",
		"the list has a group per host, each under its name",
	);
	check(
		(await group("laptop").locator(".list-empty").innerText()) ===
			"No sessions on laptop yet.",
		"an empty host says so by name",
	);
	check(
		(await savedHosts(page))
			.map((host) => host.name)
			.toSorted()
			.join(", ") === "desk, laptop",
		"local storage keeps both hosts, with the names they gave",
	);
	await back();
	await page.getByRole("button", { name: "New session" }).click();
	check(
		(await page.locator('select[name="host"] option').allInnerTexts()).join(
			", ",
		) === "desk, laptop",
		"the form picks a host first, from the paired ones",
	);
	await startSession("Claude Code", `${dir}/desk`, "desk");
	const where = pane.locator(".pane-title p");
	check(
		(await where.innerText()) === `${dir}/desk on desk`,
		"the session pane names its host",
	);
	await startSession("Claude Code", `${dir}/laptop`, "laptop");
	check(
		(await where.innerText()) === `${dir}/laptop on laptop`,
		"the second session runs on the other host",
	);
	await back();
	await page.getByRole("button", { name: "New session" }).click();
	check(
		(await page
			.locator('select[name="host"]')
			.evaluate(
				(select: HTMLSelectElement) => select.selectedOptions[0]?.label,
			)) === "laptop",
		"a new session starts on the host of the session shown last",
	);
	await back();
	check(
		(await rowsOf("desk")) === "desk" && (await rowsOf("laptop")) === "laptop",
		"each session is listed under its own host",
	);
	await page.screenshot({ path: out });
	log(`screenshot ${out}`);
};

export const hosts = async (run: Run): Promise<void> => {
	const { page, joinUrl, patience } = run;
	const { check, back, waitFor } = tools(run);
	const { rowsOf, names, bothLive, pair } = listOf(run);
	await pairedAndStarted(run);

	await page.reload();
	await bothLive();
	check(
		(await rowsOf("desk")) === "desk" && (await rowsOf("laptop")) === "laptop",
		"both hosts and their sessions are back after a reload",
	);

	// desk's join link again replaces its ticket; it stays one host.
	await pair(joinUrl);
	check(
		(await names()) === "desk, laptop" && (await rowsOf("desk")) === "desk",
		"pairing desk again keeps one desk and its session",
	);

	await page.getByRole("button", { name: "Credentials" }).click();
	check(
		(await page.locator(".host-credentials h2").allInnerTexts()).join(", ") ===
			"desk, laptop",
		"the credentials cover each host, grouped by host",
	);
	await page.getByRole("button", { name: "Forget laptop" }).click();
	await back();
	await page
		.locator(".host-group")
		.nth(1)
		.waitFor({ state: "detached", timeout: patience });
	check((await names()) === "desk", "forgetting laptop drops its group");
	await page.reload();
	await waitFor(LIVE);
	check(
		(await names()) === "desk" && (await rowsOf("desk")) === "desk",
		"laptop stays forgotten after a reload",
	);
};
