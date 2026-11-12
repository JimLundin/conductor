// Drives the web build at an iPhone viewport against a live `conductord host`
// on the fake CLIs, through n0's relays and then WebRTC. ENGINE picks
// chromium or webkit; webkit stands in for iOS Safari, which Chromium does
// not catch (it accepted the relay's root-dot hostname that Safari rejected).
//
// usage: ENGINE=webkit node browser.mjs <join-url> <out> <scenario> <dir>
//
// Scenarios, each against its own host. <dir> is the session directory on
// the host, holding the fake's recording; <out> is a screenshot, or for
// screens the directory the screenshots go to.
//   upgrade   Live on the relay, then WebRTC, a Claude turn over WebRTC,
//             reload, and pairing a fresh install.
//   fallback  The host drops every WebRTC datagram: the upgrade fails and a
//             turn completes on the relay.
//   drop      The host drops WebRTC mid-turn: the client falls back to the
//             relay, sees every event once, and upgrades again.
//   codex     Two Codex sessions in <dir>/first and <dir>/second, one
//             approved and one denied.
//   screens   Sessions in <dir>/conductor (held mid-reply), <dir>/scratch
//             (the CLI hangs up) and <dir>/notes (never sent to), shot at
//             1280x800 and 390x844, light and dark.

import { chromium, webkit } from 'playwright';

const [joinUrl, out, scenario, dir] = process.argv.slice(2);
if (!dir) {
  console.error('usage: node browser.mjs <join-url> <out> <upgrade|fallback|drop|codex|screens> <dir>');
  process.exit(2);
}

const UPGRADE_WITHIN_MS = 10_000;

const t0 = Date.now();
const log = (line) => console.log(`[${((Date.now() - t0) / 1000).toFixed(3).padStart(7)}s] ${line}`);
const check = (ok, what) => {
  if (!ok) throw new Error(`FAILED: ${what}`);
  log(`ok: ${what}`);
};

const engine = process.env.ENGINE ?? 'chromium';
const browser = await { chromium, webkit }[engine].launch();
log(`engine: ${engine} ${browser.version()}, scenario: ${scenario}`);
const context = await browser.newContext({
  viewport: { width: 390, height: 844 },
  deviceScaleFactor: 3,
  isMobile: true,
  hasTouch: true,
});
// Records every status change with the page's own clock, so upgrade timings
// don't depend on how often Playwright polls.
const recordStatus = () => {
  window.__status = [];
  let last = '';
  new MutationObserver(() => {
    const header = document.querySelector('header.status');
    if (!header) return;
    const now = `${header.dataset.state} ${header.dataset.path}`;
    if (now !== last) window.__status.push({ t: performance.now(), state: header.dataset.state, path: header.dataset.path });
    last = now;
  }).observe(document, { subtree: true, childList: true, attributes: true });
};
await context.addInitScript(recordStatus);
const page = await context.newPage();
/// Milliseconds from the first Live on the relay to the first Live on WebRTC.
const upgradeMs = () =>
  page.evaluate(() => {
    const relayed = window.__status.find((s) => s.state === 'Live' && s.path === 'Relayed');
    const rtc = window.__status.find((s) => s.state === 'Live' && s.path === 'WebRtc');
    return relayed && rtc ? Math.round(rtc.t - relayed.t) : null;
  });
const consoleErrors = [];
const linkErrors = [];
const fallbacks = [];
const watch = (p) => {
  p.on('console', (msg) => {
    const text = msg.text();
    if (msg.type() === 'error') consoleErrors.push(text);
    if (/link status/.test(text) && /error = Some/.test(text)) linkErrors.push(text.replace(/%c|color:.*$/gs, ''));
    if (/WebRTC dropped; back on iroh/.test(text)) fallbacks.push(Date.now());
  });
  p.on('pageerror', (err) => consoleErrors.push(String(err)));
};
watch(page);

const status = page.locator('header.status');
const statusLine = async () => (await status.innerText()).replace(/\s+/g, ' ');
const path = () => status.getAttribute('data-path');
const waitFor = (selector, timeout) => page.locator(selector).waitFor({ timeout });
const waitPath = async (kind, timeout) => {
  const start = Date.now();
  await waitFor(`header.status[data-state="Live"][data-path="${kind}"]`, timeout);
  return Date.now() - start;
};

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
  log(`started a ${agent} session in ${directory} on ${await path()}`);
};
const send = async (prompt) => {
  const composer = pane.locator('.composer textarea');
  await composer.fill(prompt);
  await composer.press('Enter');
  log(`sent "${prompt}" on ${await path()}`);
};
const text = async (selector) => (await pane.locator(selector).allInnerTexts()).join(' | ');

// fixtures/e2e/claude-approve.jsonl: one Write, approved, then the reply.
const PROMPT = 'make a file';
const WRITTEN = '/tmp/s1-scratch/a.txt';
const REPLY = 'I created `a.txt` in `/tmp/s1-scratch` with the content `hello from s1`.';

/// A Claude session in `dir`, one message through approval to the end of the
/// turn. The recording asks before it replies, so the reply streams after
/// Approve. `during` runs once the approval card shows, `beforeApprove` after
/// it. Checks the whole turn rendered exactly once.
const turn = async ({ during = async () => {}, beforeApprove = async () => {} } = {}) => {
  await startSession('Claude Code', dir);
  await send(PROMPT);
  const approve = pane.locator('.request[data-open="true"] button.approve');
  await approve.waitFor({ timeout: 30_000 });
  log(`approval card: "${(await pane.locator('.request[data-open="true"]').innerText()).replace(/\s+/g, ' ')}"`);
  await during();
  await beforeApprove();
  await approve.tap();
  log(`tapped Approve on ${await path()}`);
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
  check((await pane.locator('.tool').getAttribute('data-state')) === 'succeeded', 'the tool succeeded');
  check((await text('.request .resolved')) === 'Approved by browser', 'the approval resolved once, by this browser');
  check((await text('.reply')) === REPLY, 'the streamed reply is every delta exactly once');
  check((await text('.turn-end')) === 'Turn finished', 'exactly one "Turn finished"');
};

log(`open ${joinUrl.replace(/#join=.*/, '#join=<ticket>')}`);
const opened = Date.now();
await page.goto(joinUrl);
await waitFor('header.status[data-state="Live"]', 60_000);
log(`Live ${Date.now() - opened}ms after navigation (first connect)`);
await waitFor('header.status[data-state="Live"]:not([data-path="Unknown"])', 10_000);
log(`status line: "${await statusLine()}"`);

if (scenario === 'upgrade') {
  check(!page.url().includes('#join='), 'the join fragment was removed from the address bar');
  check(
    (await page.evaluate(() => localStorage.getItem('conductor.ticket'))) !== null,
    'the ticket is in local storage',
  );
  await waitPath('WebRtc', UPGRADE_WITHIN_MS);
  log(`WebRtc ${await upgradeMs()}ms after Live on the relay (page clock)`);
  log(`status line: "${await statusLine()}"`);
  log(`selected candidate pair (local/remote): ${await status.getAttribute('data-candidates')}`);

  await turn({
    beforeApprove: async () => {
      for (const control of ['.request button.approve', '.request button.deny', '.composer textarea', '.composer button']) {
        const box = await pane.locator(control).last().boundingBox();
        check(box.height >= 44 && box.width >= 44, `${control} is at least 44px (${box.width.toFixed(0)}x${box.height.toFixed(0)})`);
      }
    },
  });
  check((await path()) === 'WebRtc', 'the turn ended still on WebRTC');
  check(fallbacks.length === 0, 'WebRTC never dropped during the turn');
  await page.screenshot({ path: out });
  log(`screenshot ${out}`);

  const reloaded = Date.now();
  await page.goto(joinUrl.replace(/#join=.*/, ''));
  await waitFor('header.status[data-state="Live"]', 60_000);
  log(`Live ${Date.now() - reloaded}ms after reload without the fragment (reconnect)`);
  await openRow(dir.split('/').pop());
  await pane.locator('.turn-end').waitFor({ timeout: 10_000 });
  check((await text('.reply')) === REPLY, 'the transcript replayed after reload');
  await waitPath('WebRtc', UPGRADE_WITHIN_MS);
  log(`WebRtc ${await upgradeMs()}ms after Live on the relay, after reload (page clock)`);

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
} else if (scenario === 'fallback') {
  check((await path()) === 'Relayed', 'Live on the relay');
  await turn();
  check((await path()) === 'Relayed', 'the turn completed on the relay');
  const failed = page.locator('header.status[data-path="Relayed"]:not([data-upgrade-error=""])');
  await failed.waitFor({ timeout: 20_000 });
  log(`upgrade error shown to the client: "${await status.getAttribute('data-upgrade-error')}"`);
  check((await status.getAttribute('data-state')) === 'Live', 'still Live after the upgrade failed');
  await page.screenshot({ path: out });
} else if (scenario === 'drop') {
  await waitPath('WebRtc', UPGRADE_WITHIN_MS);
  log(`WebRtc ${await upgradeMs()}ms after Live on the relay (page clock)`);
  // The drop lands between the message, sent on WebRTC, and the approval card.
  await turn({
    during: async () => {
      check(fallbacks.length === 1, 'WebRTC dropped mid-turn, before the approval card');
      const relayed = await page.evaluate(() => window.__status.some((s, i) => i > 0 && s.state === 'Live' && s.path === 'Relayed'));
      check(relayed, 'the client went back to Live on the relay');
    },
  });
  check(fallbacks.length === 1, 'the client fell back exactly once');
  await waitPath('WebRtc', 30_000);
  log(`WebRtc again ${Date.now() - fallbacks[0]}ms after the drop`);
  await page.screenshot({ path: out });
} else if (scenario === 'codex') {
  // fixtures/e2e/codex-two-threads.jsonl: two threads start, then a turn on
  // each; the first asks to run a command and is approved, the second is
  // declined.
  const [first, second] = [`${dir}/first`, `${dir}/second`];
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
  check((await text('.request .resolved')) === 'Approved by browser', 'the first request was approved by this browser');
  check((await pane.locator('.tool').getAttribute('data-state')) === 'succeeded', 'the approved command succeeded');
  check((await text('.reply')).endsWith('Created [a.txt](/tmp/s2-scratch/a.txt).'), 'the first turn ended with its own answer');
  await openRow('second');
  await pane.locator('.request[data-open="true"] button.deny').click();
  await pane.locator('.turn-end[data-outcome="completed"]').waitFor({ timeout: 20_000 });
  check((await text('.request .resolved')) === 'Denied by browser', 'the second request was denied by this browser');
  check((await text('.reply')) === 'The request was declined.', 'the second turn ended with its own answer');
  await page.screenshot({ path: out });
} else if (scenario === 'screens') {
  await startSession('Claude Code', `${dir}/notes`);
  await startSession('Claude Code', `${dir}/scratch`);
  await pane.locator('.closed-note').waitFor({ timeout: 20_000 });
  log(`closed: "${await text('.closed-note')}"`);
  await startSession('Claude Code', `${dir}/conductor`);
  await send(PROMPT);
  await pane.locator('.request[data-open="true"] button.approve').click();
  await pane.locator('.reply.streaming', { hasText: '/tmp/s1' }).waitFor({ timeout: 20_000 });
  log(`running, held mid-reply: "${await text('.reply')}"`);
  const shots = {
    desktop: [{ viewport: { width: 1280, height: 800 } }, [['conductor', 'running'], ['scratch', 'closed']]],
    phone: [
      { viewport: { width: 390, height: 844 }, deviceScaleFactor: 2, isMobile: true, hasTouch: true },
      [['conductor', 'running'], ['notes', 'idle']],
    ],
  };
  for (const scheme of ['light', 'dark']) {
    for (const [device, [options, states]] of Object.entries(shots)) {
      const shooter = await browser.newContext({ ...options, colorScheme: scheme, reducedMotion: 'reduce' });
      const shot = await shooter.newPage();
      watch(shot);
      await shot.goto(joinUrl);
      await shot.locator('header.status[data-state="Live"]').waitFor({ timeout: 60_000 });
      for (const [title, state] of states) {
        await openRow(title, shot);
        await shot.waitForTimeout(450);
        const file = `${out}/${device}-${scheme}-${state}.png`;
        await shot.screenshot({ path: file });
        log(`screenshot ${file}`);
      }
      await shooter.close();
    }
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
