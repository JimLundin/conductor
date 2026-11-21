import type { Locator, Page } from "playwright";
import {
	JOIN_FRAGMENT,
	LIVE,
	type Run,
	savedHosts,
	tools,
} from "../harness.ts";

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
	await pairingPersistence(run);
};

const concurrentPairings = async (
	run: Run,
	first: Page,
	second: Page,
): Promise<void> => {
	const { check } = tools(run);
	const { joinUrl, secondJoin, patience } = run;
	if (secondJoin === undefined) {
		throw new Error("FAILED: the hosts scenario needs SECOND_JOIN");
	}
	const root = joinUrl.replace(JOIN_FRAGMENT, "");
	await Promise.all([first.goto(root), second.goto(root)]);
	const firstTicket = first.locator("form.pair input");
	const secondTicket = second.locator("form.pair input");
	await firstTicket.fill("bad ticket");
	await firstTicket.press("Enter");
	await first
		.getByText("That join link is not valid", { exact: false })
		.waitFor({
			timeout: patience,
		});
	await Promise.all([
		firstTicket.fill(new URL(joinUrl).hash.slice("#join=".length)),
		secondTicket.fill(secondJoin),
	]);
	await Promise.all([firstTicket.press("Enter"), secondTicket.press("Enter")]);
	await Promise.all([
		first.locator(LIVE).waitFor({ timeout: patience }),
		second.locator(LIVE).waitFor({ timeout: patience }),
	]);
	check(
		(await savedHosts(first)).length === 2,
		"two independent pairing writes both survive",
	);
	await first.getByRole("button", { name: "Credentials" }).click();
	await first.getByRole("button", { name: "Forget desk" }).click();
	await second.reload();
	await second.locator(LIVE).waitFor({ timeout: patience });
	check(
		(await second.locator(".host-group .status .host").allInnerTexts()).join(
			", ",
		) === "laptop",
		"the other tab sees the intentional removal on reload",
	);
};

const storageRecovery = async (run: Run, page: Page): Promise<void> => {
	const { check } = tools(run);
	const url = new URL(run.joinUrl);
	const ticket = url.hash.slice("#join=".length);
	url.searchParams.set("storage-test", "preserve");
	await page.addInitScript((legacy) => {
		// Set up the failure before the app can read the hash or storage.
		// sessionStorage keeps this a one-time setup across localStorage clears.
		if (sessionStorage.getItem("test.storage-seeded") !== "1") {
			localStorage.clear();
			localStorage.setItem("conductor.ticket", legacy);
			localStorage.setItem("test.deny-writes", "1");
			sessionStorage.setItem("test.storage-seeded", "1");
		}
		const write = Storage.prototype.setItem;
		Object.defineProperty(Storage.prototype, "setItem", {
			value(this: Storage, key: string, value: string): void {
				if (
					this.getItem("test.deny-writes") === "1" &&
					key.startsWith("conductor.")
				) {
					throw new DOMException("Test storage denial", "QuotaExceededError");
				}
				write.call(this, key, value);
			},
		});
	}, ticket);
	await page.goto(url.href);
	const storageError = page.locator(".storage-error");
	await storageError.waitFor({ timeout: run.patience });
	await page.reload();
	await storageError.waitFor({ timeout: run.patience });
	check(
		new URL(page.url()).hash === url.hash,
		"failed join persistence keeps the original join fragment",
	);
	check(
		await page.evaluate(
			(legacy) => localStorage.getItem("conductor.ticket") === legacy,
			ticket,
		),
		"failed legacy migration keeps the original saved ticket",
	);
	await page.evaluate(() => localStorage.removeItem("test.deny-writes"));
	await page.reload();
	await page.locator(".host-group").waitFor({ timeout: run.patience });
	check(
		new URL(page.url()).search === url.search &&
			new URL(page.url()).hash === "",
		"successful persistence consumes the join fragment and preserves the query",
	);
	check(
		await page.evaluate(
			() => localStorage.getItem("conductor.ticket") === null,
		),
		"successful migration retires the legacy ticket",
	);
	await page.evaluate(() => {
		const record = Object.keys(localStorage).find((key) =>
			key.startsWith("conductor.host."),
		);
		const healthy = record === undefined ? null : localStorage.getItem(record);
		localStorage.clear();
		localStorage.setItem(
			"conductor.hosts",
			`{${healthy?.slice(1, -1) ?? ""},"malformed":{"ticket":42}}`,
		);
	});
	await page.reload();
	await page.locator(".host-group").waitFor({ timeout: run.patience });
	await page.reload();
	await page.locator(".host-group").waitFor({ timeout: run.patience });
	check(
		(await page.locator(".host-group").count()) === 1,
		"a healthy saved host survives malformed neighbors and repeated reloads",
	);
};

const pairingPersistence = async (run: Run): Promise<void> => {
	const browser = run.page.context().browser();
	if (browser === null) {
		throw new Error("FAILED: pairing persistence needs a browser");
	}
	const context = await browser.newContext();
	try {
		const first = await context.newPage();
		const second = await context.newPage();
		await concurrentPairings(run, first, second);
		await second.close();
		await storageRecovery(run, first);
	} finally {
		await context.close();
	}
};
