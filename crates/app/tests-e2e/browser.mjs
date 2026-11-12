// Drives the web build at an iPhone viewport against a live `conductord host`
// on the fake CLIs, through n0's relays. ENGINE picks
// chromium or webkit; webkit stands in for iOS Safari, which Chromium does
// not catch (it accepted the relay's root-dot hostname that Safari rejected).
//
// usage: ENGINE=webkit node browser.mjs <join-url> <out> <scenario> <dir>
//
// Scenarios, each against its own host. <dir> is the session directory on
// the host, holding the fake's recording; <out> is a screenshot, or for
// screens the directory the screenshots go to.
//   turn      Live on the relay, a Claude turn through approval, reload,
//             and pairing a fresh install.
//   codex     Two Codex sessions in <dir>/first and <dir>/second, one
//             approved and one denied.
//   credentials  A Claude gateway entered, shown as configured, and
//             cleared; <dir> is unused.
//   screens   Sessions in <dir>/conductor (held mid-reply), <dir>/scratch
//             (the CLI hangs up) and <dir>/notes (never sent to), shot at
//             1280x800 and 390x844, light and dark.

import { chromium, webkit } from 'playwright';

const [joinUrl, out, scenario, dir] = process.argv.slice(2);
if (!dir) {
  console.error('usage: node browser.mjs <join-url> <out> <turn|codex|screens> <dir>');
  process.exit(2);
}

const t0 = Date.now();
const log = (line) => console.log(`[${((Date.now() - t0) / 1000).toFixed(3).padStart(7)}s] ${line}`);
const check = (ok, what) => {
  if (!ok) throw new Error(`FAILED: ${what}`);
  log(`ok: ${what}`);
};

const engine = process.env.ENGINE ?? 'chromium';
const browser = await { chromium, webkit }[engine].launch();
log(`engine: ${engine} ${browser.version()}, scenario: ${scenario}`);
// An iPhone's user agent, so the app names itself "iPhone" in Hello.
const IPHONE_UA =
  'Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Mobile/15E148 Safari/604.1';
const context = await browser.newContext({
  viewport: { width: 390, height: 844 },
  deviceScaleFactor: 3,
  isMobile: true,
  hasTouch: true,
  userAgent: IPHONE_UA,
});
const page = await context.newPage();
const consoleErrors = [];
const linkErrors = [];
const watch = (p) => {
  p.on('console', (msg) => {
    const text = msg.text();
    if (msg.type() === 'error') consoleErrors.push(text);
    if (/link status/.test(text) && /error = Some/.test(text)) linkErrors.push(text.replace(/%c|color:.*$/gs, ''));
  });
  p.on('pageerror', (err) => consoleErrors.push(String(err)));
};
watch(page);

const status = page.locator('header.status');
const statusLine = async () => (await status.innerText()).replace(/\s+/g, ' ');
const waitFor = (selector, timeout) => page.locator(selector).waitFor({ timeout });

const pane = page.locator('.session-pane');
/// Back to the session list; only a phone has the button.
const back = async (p = page) => {
  const button = p.locator('.pane-head .back');
  if (await button.isVisible()) await button.click();
};
const openRow = async (title, p = page) => {
  await back(p);
  await p.locator('.session-row', { has: p.locator('.row-title', { hasText: new RegExp(`^${title}$`) }) }).click();
  await p.locator('.session-pane').waitFor({ timeout: 10_000 });
};
/// Starts a session from the New session form and waits until it takes a
/// message or has closed.
const startSession = async (agent, directory) => {
  await back();
  await page.getByRole('button', { name: 'New session' }).click();
  await page.locator('.segment', { hasText: agent }).click();
  await page.locator('input[name="directory"]').fill(directory);
  await page.getByRole('button', { name: 'Start session' }).click();
  await pane.locator(`.composer textarea[placeholder="Message ${agent}"], .closed-note`).waitFor({ timeout: 20_000 });
  log(`started a ${agent} session in ${directory}`);
};
const send = async (prompt) => {
  const composer = pane.locator('.composer textarea');
  await composer.fill(prompt);
  await composer.press('Enter');
  log(`sent "${prompt}"`);
};
const text = async (selector) => (await pane.locator(selector).allInnerTexts()).join(' | ');

// fixtures/e2e/claude-approve.jsonl: one Write, approved, then the reply.
const PROMPT = 'make a file';
const WRITTEN = '/tmp/s1-scratch/a.txt';
const REPLY = 'I created `a.txt` in `/tmp/s1-scratch` with the content `hello from s1`.';

/// A Claude session in `dir`, one message through approval to the end of the
/// turn. The recording asks before it replies, so the reply streams after
/// Approve. `beforeApprove` runs once the approval card shows. Checks the
/// whole turn rendered exactly once.
const turn = async ({ beforeApprove = async () => {} } = {}) => {
  await startSession('Claude Code', dir);
  await send(PROMPT);
  const approve = pane.locator('.request[data-open="true"] button.approve');
  await approve.waitFor({ timeout: 30_000 });
  log(`approval card: "${(await pane.locator('.request[data-open="true"]').innerText()).replace(/\s+/g, ' ')}"`);
  await beforeApprove();
  await approve.tap();
  log('tapped Approve');
  const reply = pane.locator('.reply.streaming');
  await reply.waitFor({ timeout: 20_000 });
  const seen = new Set();
  while (seen.size < 3 && (await reply.count()) > 0) {
    seen.add(await reply.innerText().catch(() => ''));
    await page.waitForTimeout(40);
  }
  check(seen.size >= 3, `deltas rendered live (${seen.size} distinct partial texts seen)`);
  await pane.locator('.turn-end[data-outcome="completed"]').waitFor({ timeout: 30_000 });
  check((await text('.user .bubble')) === PROMPT, 'the message shows once');
  check((await text('.tool .tool-detail')) === WRITTEN, 'one tool card, for the written file');
  check((await text('.tool .tool-name')) === 'Write', 'the card names the change a Write');
  check((await pane.locator('.tool').getAttribute('data-state')) === 'succeeded', 'the tool succeeded');
  check((await text('.request .resolved')) === 'Approved by iPhone', 'the approval resolved once, by this device');
  check((await text('.reply')) === REPLY, 'the streamed reply is every delta exactly once');
  check((await text('.turn-end')) === 'Turn finished', 'exactly one "Turn finished"');
};

log(`open ${joinUrl.replace(/#join=.*/, '#join=<ticket>')}`);
const opened = Date.now();
await page.goto(joinUrl);
await waitFor('header.status[data-state="Live"]', 60_000);
log(`Live ${Date.now() - opened}ms after navigation (first connect)`);
log(`status line: "${await statusLine()}"`);

if (scenario === 'turn') {
  check(!page.url().includes('#join='), 'the join fragment was removed from the address bar');
  check(
    (await page.evaluate(() => localStorage.getItem('conductor.ticket'))) !== null,
    'the ticket is in local storage',
  );
  // This host runs no Codex CLI.
  await page.getByRole('button', { name: 'New session' }).click();
  await page.locator('.segment', { hasText: 'Codex' }).click();
  const why = await page.locator('.providers .hint').innerText();
  check(why.startsWith("Codex can't run on this host: "), `the form says why Codex is unavailable ("${why}")`);

  await turn({
    beforeApprove: async () => {
      for (const control of ['.request button.approve', '.request button.deny', '.composer textarea', '.composer button']) {
        const box = await pane.locator(control).last().boundingBox();
        check(box.height >= 44 && box.width >= 44, `${control} is at least 44px (${box.width.toFixed(0)}x${box.height.toFixed(0)})`);
      }
    },
  });
  await page.screenshot({ path: out });
  log(`screenshot ${out}`);

  // Playwright can neither open the keyboard nor give the page a home
  // indicator, so the inset is set by hand; the reload below drops it.
  await page.addStyleTag({ content: '.composer { --inset-bottom: 34px; }' });
  const padding = () => pane.locator('.composer').evaluate((el) => getComputedStyle(el).paddingBottom);
  check((await padding()) === '42px', 'the composer keeps the home indicator inset with the keyboard down');
  await pane.locator('.composer textarea').tap();
  check((await padding()) === '8px', 'the composer drops the inset while it has focus, as with the keyboard up');

  const reloaded = Date.now();
  await page.goto(joinUrl.replace(/#join=.*/, ''));
  await waitFor('header.status[data-state="Live"]', 60_000);
  log(`Live ${Date.now() - reloaded}ms after reload without the fragment (reconnect)`);
  await openRow(dir.split('/').pop());
  await pane.locator('.turn-end').waitFor({ timeout: 10_000 });
  check((await text('.reply')) === REPLY, 'the transcript replayed after reload');

  // A Home Screen web app on iOS starts with empty storage and no fragment, so it
  // must pair by pasting the join link.
  const fresh = await browser.newContext({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  const installed = await fresh.newPage();
  installed.on('console', (msg) => msg.type() === 'error' && consoleErrors.push(msg.text()));
  await installed.goto(joinUrl.replace(/#join=.*/, ''));
  await installed.getByRole('heading', { name: 'Connect to a host' }).waitFor({ timeout: 30_000 });
  check(true, 'a fresh install asks for a join link');
  await installed.locator('form.pair input').fill(joinUrl);
  const pasted = Date.now();
  await installed.locator('form.pair button').click();
  await installed.locator('header.status[data-state="Live"]').waitFor({ timeout: 60_000 });
  log(`Live ${Date.now() - pasted}ms after pasting the join link`);
  // The camera can't hand a new join link to a Home Screen app, so a paired
  // app must be able to drop its host and pair again from inside.
  await installed.getByRole('button', { name: 'Change host' }).click();
  await installed.getByRole('heading', { name: 'Connect to a host' }).waitFor({ timeout: 10_000 });
  await installed.reload();
  await installed.getByRole('heading', { name: 'Connect to a host' }).waitFor({ timeout: 30_000 });
  check(true, 'Change host forgets the saved host, across a relaunch');
  await installed.locator('form.pair input').fill(joinUrl);
  await installed.locator('form.pair button').click();
  await installed.locator('header.status[data-state="Live"]').waitFor({ timeout: 60_000 });
  check(true, 'pairing again after Change host reaches Live');
  await fresh.close();
} else if (scenario === 'codex') {
  // fixtures/e2e/codex-two-threads.jsonl: two threads start, then a turn on
  // each; the first asks to run a command and is approved, the second is
  // declined.
  const [first, second] = [`${dir}/first`, `${dir}/second`];
  await page.getByRole('button', { name: 'New session' }).click();
  await page.locator('.segment', { hasText: 'Codex' }).click();
  check(
    (await page.locator('.providers .hint').innerText()) ===
      'Codex runs commands and edits files in this folder and in /tmp without asking. It asks before writing anywhere else, changing .git, or using the network.',
    'choosing Codex says what it does without asking',
  );
  await startSession('Codex', first);
  await startSession('Codex', second);
  await openRow('first');
  await send('write a.txt');
  await pane.locator('.request[data-open="true"]').waitFor({ timeout: 20_000 });
  check((await text('.reply')) === 'I’ll create `a.txt` with the exact requested contents.', 'the first reply streamed in before the request');
  await openRow('second');
  await send('write b.txt');
  await pane.locator('.request[data-open="true"]').waitFor({ timeout: 20_000 });
  log(`second approval card: "${(await pane.locator('.request[data-open="true"]').innerText()).replace(/\s+/g, ' ')}"`);
  await openRow('first');
  await pane.locator('.request[data-open="true"] button.approve').click();
  await pane.locator('.turn-end[data-outcome="completed"]').waitFor({ timeout: 20_000 });
  check((await text('.request .resolved')) === 'Approved by iPhone', 'the first request was approved by this device');
  check((await pane.locator('.tool').getAttribute('data-state')) === 'succeeded', 'the approved command succeeded');
  check((await text('.reply')).endsWith('Created [a.txt](/tmp/s2-scratch/a.txt).'), 'the first turn ended with its own answer');
  await openRow('second');
  await pane.locator('.request[data-open="true"] button.deny').click();
  await pane.locator('.turn-end[data-outcome="completed"]').waitFor({ timeout: 20_000 });
  check((await text('.request .resolved')) === 'Denied by iPhone', 'the second request was denied by this device');
  check((await text('.reply')) === 'The request was declined.', 'the second turn ended with its own answer');
  await page.screenshot({ path: out });
} else if (scenario === 'credentials') {
  const GATEWAY = 'https://gateway.conductor-test.invalid';
  const TOKEN = 'conductor-test-browser-token';
  await page.getByRole('button', { name: 'Credentials' }).click();
  const claude = page.getByRole('form', { name: 'Claude Code' });
  const state = claude.locator('.credential-state');
  check((await state.innerText()) === "The host's own CLI, signed in on its own", 'Claude starts on its own CLI');
  check(
    (await page.getByRole('form', { name: 'Codex' }).locator('.credential-state').innerText()).startsWith('Not available:'),
    'Codex is unavailable on this host'
  );
  const token = claude.getByLabel('Token');
  check((await token.getAttribute('type')) === 'password', 'the token field is a password field');
  await claude.getByLabel('Gateway URL').fill(GATEWAY);
  await token.fill(TOKEN);
  await claude.getByRole('button', { name: 'Save gateway' }).click();
  await state.filter({ hasText: `Gateway at ${GATEWAY}` }).waitFor({ timeout: 10_000 });
  log(`state: "${await state.innerText()}"`);
  check((await token.inputValue()) === '', 'the token field is empty once sent');
  check(
    (await claude.getByRole('status').innerText()) === "Sent to conductor over the encrypted link. This app doesn't keep it; your browser may offer to save it.",
    'the form says the app keeps nothing'
  );
  check((await token.getAttribute('autocomplete')) === 'new-password', 'the token field asks a password manager not to fill it');
  const kept = await page.evaluate(async () =>
    JSON.stringify([
      { ...localStorage },
      { ...sessionStorage },
      location.href,
      document.documentElement.outerHTML,
      await indexedDB.databases(),
    ])
  );
  check(!kept.includes(TOKEN), 'the token is in no storage, the URL or the DOM');
  await page.screenshot({ path: out });
  await claude.getByRole('button', { name: 'Clear the Claude Code credential' }).click();
  await state.filter({ hasText: "The host's own CLI" }).waitFor({ timeout: 10_000 });
  check((await claude.getByRole('status').innerText()) === 'Cleared on conductor.', 'the clear is confirmed');
  check((await page.evaluate(() => document.activeElement?.id)) === 'claude-credential', 'focus moves to the Claude heading');
} else if (scenario === 'screens') {
  await startSession('Claude Code', `${dir}/notes`);
  await startSession('Claude Code', `${dir}/scratch`);
  await pane.locator('.closed-note').waitFor({ timeout: 20_000 });
  log(`closed: "${await text('.closed-note')}"`);
  await startSession('Claude Code', `${dir}/conductor`);
  await send(PROMPT);
  const approve = pane.locator('.request[data-open="true"] button.approve');
  await approve.waitFor({ timeout: 20_000 });
  const shots = {
    desktop: [{ viewport: { width: 1280, height: 800 } }, [['conductor', 'running'], ['scratch', 'closed']]],
    phone: [
      { viewport: { width: 390, height: 844 }, deviceScaleFactor: 2, isMobile: true, hasTouch: true },
      [['conductor', 'running'], ['notes', 'idle']],
    ],
  };
  // Deltas are live-only, so every shooter is Live before the reply starts.
  const shooters = [];
  for (const scheme of ['light', 'dark']) {
    for (const [device, [options, states]] of Object.entries(shots)) {
      const context = await browser.newContext({ ...options, colorScheme: scheme, reducedMotion: 'reduce' });
      const shot = await context.newPage();
      watch(shot);
      await shot.goto(joinUrl);
      await shot.locator('header.status[data-state="Live"]').waitFor({ timeout: 60_000 });
      shooters.push({ scheme, device, states, context, shot });
    }
  }
  await approve.click();
  await pane.locator('.reply.streaming', { hasText: '/tmp/s1' }).waitFor({ timeout: 20_000 });
  log(`running, held mid-reply: "${await text('.reply')}"`);
  for (const { scheme, device, states, context, shot } of shooters) {
    for (const [title, state] of states) {
      await openRow(title, shot);
      if (state === 'running') {
        await shot.locator('.reply.streaming', { hasText: '/tmp/s1' }).waitFor({ timeout: 10_000 });
      }
      await shot.waitForTimeout(450);
      const file = `${out}/${device}-${scheme}-${state}.png`;
      await shot.screenshot({ path: file });
      log(`screenshot ${file}`);
    }
    await context.close();
  }
} else {
  throw new Error(`unknown scenario ${scenario}`);
}

check(linkErrors.length === 0, `the link never reported an error${linkErrors.length ? `: ${linkErrors.join(' | ')}` : ''}`);
// n0's public relays are best-effort: a correctly named relay can reset a
// WebSocket handshake, and the client retries. Those are reported, not failed.
// A relay named with the root dot is still a failure (Safari rejects it).
const relayHiccup = /^WebSocket connection to 'wss:\/\/[a-z0-9-]+\.relay\.n0\.iroh\.link\/relay' failed/;
const dottedRelay = /relay\.n0\.iroh\.link\.\//;
const hiccups = consoleErrors.filter((e) => relayHiccup.test(e) && !dottedRelay.test(e));
// WebKit, like iOS Safari, ignores the viewport's `interactive-widget`, which
// Chromium uses to keep the composer above the keyboard.
const ignoredViewport = /^Viewport argument key "interactive-widget" not recognized and ignored\.$/;
const notices = consoleErrors.filter((e) => ignoredViewport.test(e));
const realErrors = consoleErrors.filter((e) => !hiccups.includes(e) && !notices.includes(e));
if (hiccups.length) log(`warning: ${hiccups.length} relay connection hiccup(s), retried by the client: ${hiccups[0]}`);
if (notices.length) log(`note: ${notices.length} console notice(s) that the viewport's interactive-widget is ignored`);
check(!consoleErrors.some((e) => dottedRelay.test(e)), 'no relay dialled with the root dot');
check(realErrors.length === 0, `no console errors${realErrors.length ? `: ${realErrors.join(' | ')}` : ''}`);
await browser.close();
log('PASS');
