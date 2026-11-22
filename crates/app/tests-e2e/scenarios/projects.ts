import type { Locator } from "playwright";
import { type Run, shotNamed, tools } from "../harness.ts";

const SYNCED =
	/^Home synced \d{4}-\d\d-\d\d \d\d:\d\d UTC from file:\/\/.*home-remote\.git\.$/u;
const SIGNED_NOTE = /^\d{4}-\d\d-\d\d conductor · iPhone: from the phone$/u;
const ISSUE_SESSION = /^Claude Code on conductor · /u;

const projectOf =
	(run: Run) =>
	(id: string): Locator =>
		run.page.locator(`.project[data-project="${id}"]`);

const blockedIssueStart = async (run: Run): Promise<void> => {
	const { page, patience } = run;
	const { back, check } = tools(run);
	const beta = projectOf(run)("beta");
	await beta.locator('input[name="issue-title"]').fill("Check beta");
	await beta.getByRole("button", { name: "Open", exact: true }).click();
	await page
		.locator(".issue-pane")
		.getByRole("button", { name: "Start a session" })
		.click();
	const form = page.locator(".new-session");
	await form
		.getByText("The issue's project is not checked out on this host.")
		.waitFor({ timeout: patience });
	check(
		await form.getByRole("button", { name: "Start session" }).isDisabled(),
		"Start is disabled for an issue without a checkout",
	);
	await back();
	await page.getByRole("button", { name: "Projects", exact: true }).click();
};

/** The home's projects: alpha found in the search root, beta cloned. */
const checkouts = async (run: Run): Promise<void> => {
	const { page, out, dir, patience, log } = run;
	const { check } = tools(run);
	const project = projectOf(run);
	await page.getByRole("button", { name: "Projects", exact: true }).click();
	await page
		.locator('.home-line[data-sync="synced"]')
		.waitFor({ timeout: patience });
	const home = await page.locator(".home-line .home-state").innerText();
	check(SYNCED.test(home), `the host's home is synced ("${home}")`);
	check(
		(await project("alpha")
			.locator('.checkout[data-checkout="checked-out"] .checkout-path')
			.innerText()) === `${dir}/roots/alpha`,
		"alpha's checkout was found in the search root, by its root commit and remote",
	);
	check(
		(await project("beta")
			.locator(".checkout")
			.getAttribute("data-checkout")) === "missing",
		"beta is not checked out",
	);
	await blockedIssueStart(run);
	await page.getByRole("button", { name: "Clone beta on conductor" }).click();
	await project("beta")
		.locator('.checkout[data-checkout="checked-out"]')
		.waitFor({ timeout: patience });
	const cloned = await project("beta").locator(".checkout-path").innerText();
	check(
		cloned.endsWith("/projects-host/projects/beta (cloned here)"),
		`beta was cloned into the host's data dir ("${cloned}")`,
	);
	await page.screenshot({ path: out });
	log(`screenshot ${out}`);
};

/** An issue opened from the project, moved on, checked and noted. */
const issue = async (run: Run): Promise<void> => {
	const { page, out, patience } = run;
	const { check, settled } = tools(run);
	const alpha = projectOf(run)("alpha");
	await alpha.locator('input[name="issue-title"]').fill("Remove sessions");
	await alpha.getByRole("button", { name: "Open", exact: true }).click();
	const pane = page.locator(".issue-pane");
	const status = pane.locator('select[name="status"]');
	await pane.waitFor({ timeout: patience });
	check(
		(await pane.locator(".pane-title h2").innerText()) === "Remove sessions",
		"the new issue opens",
	);
	check((await status.inputValue()) === "open", "it opens as open");
	await status.selectOption("active");
	await pane.locator('input[name="item"]').fill("the browser scenario");
	await pane.getByRole("button", { name: "Add", exact: true }).click();
	await pane
		.locator(".checklist li", { hasText: "the browser scenario" })
		.waitFor({ timeout: patience });
	await pane.locator('textarea[name="note"]').fill("from the phone");
	await pane.getByRole("button", { name: "Add note" }).click();
	await pane
		.locator(".note", { hasText: "from the phone" })
		.waitFor({ timeout: patience });
	const note = await pane.locator(".note").innerText();
	check(
		SIGNED_NOTE.test(note),
		`the note is dated and signed by the host and the device ("${note}")`,
	);
	check((await status.inputValue()) === "active", "the status moved on");
	await settled();
	await page.screenshot({ path: shotNamed(out, "issue") });
	await pane.getByRole("button", { name: "Back to projects" }).click();
	const active = alpha.locator('.issue-group[data-status="active"]');
	await active.locator(".issue-title").waitFor({ timeout: patience });
	check(
		(await active.locator(".issue-detail").innerText()) ===
			"normal · 0/1 done",
		"the project lists the issue under its status, with its checklist",
	);
};

/** A session started in alpha from the form's project picker. */
const projectSession = async (run: Run): Promise<void> => {
	const { page, dir, patience } = run;
	const { check, pane, back } = tools(run);
	await back();
	await page.getByRole("button", { name: "New session" }).click();
	const choices = await page
		.locator('select[name="project"] option')
		.allInnerTexts();
	check(
		choices.join(", ") === "A directory, alpha, beta",
		`the form offers the host's projects (${choices.join(", ")})`,
	);
	await page.locator(".segment", { hasText: "Claude Code" }).click();
	const form = page.locator(".new-session");
	const missing = `${dir}/missing-session-directory`;
	await form.locator('input[name="directory"]').fill(missing);
	await form.getByRole("button", { name: "Start session" }).click();
	await form
		.locator('.field:has(input[name="directory"])')
		.getByRole("alert")
		.filter({ hasText: "There is no directory at that path on conductor." })
		.waitFor({ timeout: patience });
	await form.locator('select[name="project"]').selectOption({ label: "alpha" });
	await form
		.locator(":scope > [role=alert]")
		.filter({ hasText: `There is no directory at ${missing} on conductor.` })
		.waitFor({ timeout: patience });
	check(
		(await page.locator('input[name="directory"]').count()) === 0,
		"a picked project hides the directory",
	);
	const start = page.getByRole("button", { name: "Start session" });
	check(await start.isEnabled(), "Start is enabled for a checked-out project");
	await start.click();
	await pane
		.locator('.composer textarea[placeholder="Message Claude Code"]')
		.waitFor({ timeout: patience });
	check(
		(await pane.locator(".pane-title p").innerText()) ===
			`${dir}/roots/alpha on conductor`,
		"the session runs in alpha's checkout",
	);
	await back();
	check(
		(await page.locator(".row-title").allInnerTexts()).join(", ") === "alpha",
		"the session is listed by its project",
	);
};

/** A session on the issue, started from its page: its first message is the
 * issue's brief, and the issue lists it. */
const issueSession = async (run: Run): Promise<void> => {
	const { page, patience } = run;
	const { check, pane, back } = tools(run);
	const alpha = projectOf(run)("alpha");
	const openIssue = async (): Promise<void> => {
		await page.getByRole("button", { name: "Projects", exact: true }).click();
		await alpha.locator(".issue-row", { hasText: "Remove sessions" }).click();
	};
	await openIssue();
	await page
		.locator(".issue-pane")
		.getByRole("button", { name: "Start a session" })
		.click();
	check(
		(await page.locator(".on-issue").innerText()) ===
			"On the issue Remove sessions",
		"the form starts on the issue",
	);
	check(
		(await page.locator('input[name="directory"]').count()) === 0,
		"an issue hides the directory",
	);
	await page.locator(".segment", { hasText: "Claude Code" }).click();
	await page.getByRole("button", { name: "Start session" }).click();
	const brief = pane.locator(".user .bubble", { hasText: "Remove sessions" });
	await brief.waitFor({ timeout: patience });
	check(
		(await brief.innerText()).startsWith(
			'You are working on the issue "Remove sessions" in alpha.',
		),
		"the first message is the issue's brief",
	);
	await back();
	await openIssue();
	const sessions = page.locator(".issue-sessions li");
	await sessions.first().waitFor({ timeout: patience });
	check(
		ISSUE_SESSION.test(await sessions.innerText()),
		"the issue lists its session",
	);
};

export const projects = async (run: Run): Promise<void> => {
	await checkouts(run);
	await issue(run);
	await projectSession(run);
	await issueSession(run);
};
