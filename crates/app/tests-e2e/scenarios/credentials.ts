import type { Locator } from "playwright";
import {
	claudeRegion,
	OWN_CLI,
	pageKeeps,
	type Run,
	tools,
} from "../harness.ts";

const GATEWAY = "https://gateway.conductor-test.invalid";
const TOKEN = "conductor-test-browser-token";
const NOTHING_KEPT =
	"Sent to conductor over the encrypted link. This app doesn't keep it; your browser may offer to save it.";

const labels = async (form: Locator): Promise<string> =>
	(await form.locator(".field-label").allInnerTexts()).join(", ");

/** The forms the host's catalog describes, which each provider's plugin
 * describes; each provider's own instance has a section. */
const forms = async (run: Run): Promise<void> => {
	const { page } = run;
	const { check } = tools(run);
	const claude = claudeRegion(page);
	const codex = page.getByRole("region", { name: "Codex", exact: true });
	const add = page.getByRole("region", { name: "Add a credential" });
	check(
		(await claude.locator(".credential-state").innerText()) === OWN_CLI,
		"Claude starts on its own CLI",
	);
	check(
		(await codex.locator(".credential-state").innerText()).startsWith(
			"Not available:",
		),
		"Codex is unavailable on this host",
	);
	check(
		(await labels(add.getByRole("form", { name: "Gateway" }))) ===
			"Gateway URL, Token",
		"Claude's gateway form has the fields its plugin describes",
	);
	const provider = add.getByRole("form", { name: "Model provider" });
	check(
		(await labels(provider)) === "Provider name, Base URL, Model, Key",
		"Codex's provider form has the fields its plugin describes",
	);
	check(
		(await provider.getByLabel("Key").getAttribute("type")) === "password" &&
			(await provider.getByLabel("Base URL").getAttribute("type")) === "url" &&
			(await provider
				.getByRole("button", { name: "Save model provider" })
				.isDisabled()),
		"a field kind sets its input type, and a required field left empty holds the save back",
	);
};

const checkIndexedDb = async (run: Run): Promise<void> => {
	const origin = "http://indexed-db.test/";
	const page = await run.browser.newPage();
	try {
		await page.route(origin, (route) =>
			route.fulfill({ body: "<html></html>", contentType: "text/html" }),
		);
		await page.goto(origin);
		const testKey = "synthetic-record-key";
		const testValue = "synthetic-record-value";
		await page.evaluate(
			({ key, value }) =>
				new Promise<void>((resolve, reject) => {
					const open = indexedDB.open("example");
					open.addEventListener(
						"upgradeneeded",
						() => {
							open.result
								.createObjectStore("records")
								.add({ marker: value }, key);
						},
						{ once: true },
					);
					open.addEventListener(
						"success",
						() => {
							open.result.close();
							resolve();
						},
						{ once: true },
					);
					open.addEventListener(
						"error",
						() =>
							reject(
								new Error("test database could not be populated", {
									cause: open.error,
								}),
							),
						{ once: true },
					);
				}),
			{ key: testKey, value: testValue },
		);
		const kept = await pageKeeps(page);
		tools(run).check(
			kept.includes(testKey) && kept.includes(testValue),
			"the storage check sees IndexedDB record keys and values",
		);
	} finally {
		await page.close();
	}
};

export const credentials = async (run: Run): Promise<void> => {
	await checkIndexedDb(run);
	const { page, out, patience, log } = run;
	const { check, back } = tools(run);
	await page.getByRole("button", { name: "Credentials" }).click();
	await forms(run);
	const claude = claudeRegion(page);
	const add = page.getByRole("region", { name: "Add a credential" });
	const gateway = add.getByRole("form", { name: "Gateway" });
	const token = gateway.getByLabel("Token");
	check(
		(await token.getAttribute("type")) === "password",
		"the token field is a password field",
	);
	check(
		(await token.getAttribute("autocomplete")) === "new-password",
		"the token field asks a password manager not to fill it",
	);
	const gatewayRegion = page.getByRole("region", {
		name: GATEWAY,
		exact: true,
	});
	const save = async (): Promise<void> => {
		await gateway.getByLabel("Gateway URL").fill(GATEWAY);
		await token.fill(TOKEN);
		await gateway.getByRole("button", { name: "Save gateway" }).click();
		await gatewayRegion
			.locator(".credential-state")
			.filter({ hasText: `Gateway at ${GATEWAY}` })
			.waitFor({ timeout: patience });
	};
	await save();
	log(
		`state: "${await gatewayRegion.locator(".credential-state").innerText()}"`,
	);
	check(
		(await token.inputValue()) === "",
		"the token field is empty once sent",
	);
	await add
		.getByRole("status")
		.and(add.getByText(NOTHING_KEPT, { exact: true }))
		.waitFor({ timeout: patience });
	check(
		(await gatewayRegion.locator(".credential-provider").innerText()) ===
			"Claude Code",
		"the gateway's account runs Claude Code",
	);
	check(
		(await claude.locator(".credential-state").innerText()) === OWN_CLI,
		"the gateway's account leaves Claude's own instance on its own CLI",
	);
	check(
		(await gatewayRegion.getByText("conductord login").innerText()) ===
			`To sign it in again on conductor's terminal: conductord login claude --account='${GATEWAY}'`,
		"the instance names its account for a sign-in on the host",
	);
	check(
		!(await pageKeeps(page)).includes(TOKEN),
		"the token is in no storage, the URL or the DOM",
	);
	await page.screenshot({ path: out });
	await gatewayRegion
		.getByRole("button", { name: `Remove ${GATEWAY}` })
		.click();
	await gatewayRegion.waitFor({ state: "detached", timeout: patience });
	await add
		.getByRole("status")
		.and(add.getByText("Removed from conductor.", { exact: true }))
		.waitFor({ timeout: patience });
	check(
		(await page.evaluate(() => document.activeElement?.id))?.endsWith(
			"-add-instance",
		) === true,
		"focus moves to the credential forms",
	);

	// The account's instance comes back with a credential again, and the
	// new-session picker lists it.
	await save();
	await back();
	await page.getByRole("button", { name: "New session" }).click();
	const agents = (
		await page.locator(".segment .segment-name").allInnerTexts()
	).join(", ");
	check(
		agents === `Claude Code, ${GATEWAY}, Codex`,
		`the new-session picker lists each instance (${agents})`,
	);
	const segment = page.locator(".segment", { hasText: GATEWAY });
	await segment.click();
	check(
		await segment.locator("input").isChecked(),
		"the gateway's account can be picked for a session",
	);
};
