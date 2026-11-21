import {
	claudeRegion,
	LIVE,
	OWN_CLI,
	pageKeeps,
	type Run,
	shotNamed,
	tools,
} from "../harness.ts";

// The fake's sign-in (fixtures/fake-login.py) prints Claude's link and
// takes one code; this is the one it accepts, and the account its
// status then names.
const CODE = "conductor-test-sign-in-code#fake-state";
const WRONG = "conductor-test-wrong-code";
const ACCOUNT = "conductor-test@example.invalid";
const AUTHORIZE = "https://claude.com/cai/oauth/authorize?";

/** Claude's sign-in started, its link checked, and a refused code. */
const refusedCode = async (
	run: Run,
	start: () => Promise<void>,
): Promise<void> => {
	const { page, out, patience } = run;
	const { check } = tools(run);
	const claude = claudeRegion(page);
	await start();
	const link = claude.getByRole("link", { name: "Open the sign-in page" });
	check(
		(await link.getAttribute("href"))?.startsWith(AUTHORIZE) === true &&
			(await link.getAttribute("target")) === "_blank" &&
			(await link.getAttribute("rel")) === "noopener noreferrer",
		"the link is the one the CLI printed, and opens in a new tab",
	);
	check(
		(await claude.locator('.sign-in [role="status"]').innerText()) ===
			"Open the sign-in page and sign in. Then paste the code it shows here.",
		"the page says to paste the code the sign-in page shows",
	);
	const field = claude.getByLabel("Code from the sign-in page");
	check(
		(await field.getAttribute("type")) === "password" &&
			(await claude.getByRole("button", { name: "Send code" }).isDisabled()),
		"the code field is a password field, and nothing is sent while it is empty",
	);
	await field.fill(WRONG);
	await claude.getByRole("button", { name: "Send code" }).click();
	const failed = claude.locator('.sign-in [role="alert"]');
	await failed.waitFor({ timeout: patience });
	check(
		(await failed.innerText()) ===
			"The sign-in failed: Claude didn't accept that code. Sign in again to get a new one.",
		"a refused code says so",
	);
	await page.screenshot({ path: shotNamed(out, "refused") });
};

const deniedClipboard = async (run: Run): Promise<void> => {
	const { page, patience } = run;
	const { check } = tools(run);
	await page.evaluate(() => {
		Object.defineProperty(navigator, "clipboard", {
			value: {
				writeText: () =>
					Promise.reject(
						new DOMException("Clipboard denied", "NotAllowedError"),
					),
			},
		});
	});
	await page
		.getByRole("button", { name: "Sign in: Codex", exact: true })
		.click();
	const shown = page.locator(".sign-in", {
		has: page.getByRole("button", { name: "Copy the code ABCD-12345" }),
	});
	await shown.getByRole("button", { name: "Copy the code ABCD-12345" }).click();
	await shown
		.getByRole("alert")
		.filter({ hasText: "Could not copy the code" })
		.waitFor({ timeout: patience });
	check(
		(await shown.locator(".device-code code").textContent()) === "ABCD-12345" &&
			(await shown
				.getByRole("button", { name: "Copy the code ABCD-12345" })
				.textContent()) === "Copy",
		"clipboard denial leaves the code and a retry without saying Copied",
	);
	await shown.getByRole("button", { name: "Cancel" }).click();
	await page
		.getByRole("button", { name: "Sign in: Codex", exact: true })
		.waitFor({ timeout: patience });
};

export const signIn = async (run: Run): Promise<void> => {
	const { page, out, patience, log } = run;
	const { check, waitFor } = tools(run);
	await page.getByRole("button", { name: "Credentials" }).click();
	const claude = claudeRegion(page);
	check(
		(await claude.locator(".credential-state").innerText()) === OWN_CLI,
		"Claude starts on its own CLI",
	);
	const start = async (): Promise<void> => {
		await claude.getByRole("button", { name: "Sign in: Claude Code" }).click();
		await claude
			.getByRole("link", { name: "Open the sign-in page" })
			.waitFor({ timeout: patience });
	};
	await refusedCode(run, start);
	await start();
	const field = claude.getByLabel("Code from the sign-in page");
	check(
		(await field.inputValue()) === "",
		"a new sign-in starts with an empty field",
	);
	await field.fill(CODE);
	await claude.getByRole("button", { name: "Send code" }).click();
	await field.waitFor({ state: "detached", timeout: patience });
	log("the field went once the code was sent");
	const account = page.getByRole("region", { name: ACCOUNT, exact: true });
	await account
		.locator(".credential-state")
		.filter({ hasText: "Signed in on conductor" })
		.waitFor({ timeout: patience });
	check(
		(await claude.locator('.sign-in [role="status"]').innerText()) ===
			"Signed in on conductor.",
		"the sign-in says it signed in, without a reload",
	);
	check(
		await claude
			.getByRole("button", { name: "Sign in again: Claude Code" })
			.isVisible(),
		"Claude's own instance can sign in another account",
	);
	await page.screenshot({ path: out });
	const kept = await pageKeeps(page);
	check(
		!(kept.includes(CODE) || kept.includes(WRONG)),
		"no code is in storage, the URL or the DOM",
	);
	await page.reload();
	await waitFor(LIVE);
	await page.getByRole("button", { name: "Credentials" }).click();
	check(
		(await page
			.getByRole("region", { name: ACCOUNT, exact: true })
			.locator(".credential-state")
			.innerText()) === "Signed in on conductor",
		"a reload finds the account's instance signed in",
	);
	await deniedClipboard(run);
};
