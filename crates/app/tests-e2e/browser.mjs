// Drives the web build at an iPhone viewport against a live
// `conductord s3-host`, through n0's relays and then WebRTC. ENGINE picks
// chromium or webkit; webkit stands in for iOS Safari, which Chromium does
// not catch (it accepted the relay's root-dot hostname that Safari rejected).
//
// usage: ENGINE=webkit node browser.mjs <join-url> <screenshot.png> [scenario]
//
// Scenarios, each against its own host:
//   upgrade   (default) Live on the relay, then WebRTC, a turn over WebRTC,
//             reload, and pairing a fresh install.
//   fallback  The host drops every WebRTC datagram: the upgrade fails and a
//             turn completes on the relay.
//   drop      The host drops WebRTC mid-turn: the client falls back to the
//             relay, sees every event once, and upgrades again.

import { chromium, webkit } from 'playwright';

const [joinUrl, screenshot, scenario = 'upgrade'] = process.argv.slice(2);
if (!joinUrl || !screenshot) {
  console.error('usage: node browser.mjs <join-url> <screenshot.png> [upgrade|fallback|drop]');
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

/// One prompt through approval to the end of the turn. `during` runs once
/// deltas are streaming, `beforeApprove` once the approval card shows. Checks
/// the whole turn rendered exactly once.
const turn = async (prompt, { during = async () => {}, beforeApprove = async () => {} } = {}) => {
  const turnsBefore = await page.locator('.turn-end').count();
  const input = page.locator('.composer input');
  await input.fill(prompt);
  await input.press('Enter');
  log(`submitted "${prompt}" on ${await path()}`);
  const reply = page.locator('.reply.streaming').last();
  await reply.waitFor({ timeout: 20_000 });
  const seen = new Set();
  while (seen.size < 3 && (await reply.count()) > 0) {
    seen.add(await reply.innerText().catch(() => ''));
    await page.waitForTimeout(60);
  }
  check(seen.size >= 3, `deltas rendered live (${seen.size} distinct partial texts seen)`);
  await during();
  const approve = page.locator('.approval[data-open="true"] button.approve');
  await approve.waitFor({ timeout: 30_000 });
  log(`approval card: "${(await page.locator('.approval[data-open="true"]').innerText()).replace(/\s+/g, ' ')}"`);
  await beforeApprove();
  await approve.tap();
  log(`tapped Approve on ${await path()}`);
  await page.locator('.turn-end').nth(turnsBefore).waitFor({ timeout: 30_000 });
  const n = turnsBefore;
  check(
    (await page.locator('.reply').nth(n).innerText()) === `Looking into "${prompt}". I need to run one command to answer.`,
    'the streamed reply is every delta exactly once',
  );
  check((await page.locator('.approval').count()) === n + 1, `exactly ${n + 1} approval card(s)`);
  check(
    (await page.locator('.approval .resolved').nth(n).innerText()) === 'Approved by browser',
    'the approval resolved once, by this browser',
  );
  check(
    (await page.locator('.message').nth(n).innerText()) === `Ran it. The answer to "${prompt}" is 42.`,
    'the final message arrived once',
  );
  check((await page.locator('.turn-end').count()) === n + 1, `exactly ${n + 1} "Turn finished"`);
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

  await turn('hello from the phone', {
    beforeApprove: async () => {
      for (const control of ['.approval button.approve', '.approval button.deny', '.composer input', '.composer button']) {
        const box = await page.locator(control).last().boundingBox();
        check(box.height >= 44 && box.width >= 44, `${control} is at least 44px (${box.width.toFixed(0)}x${box.height.toFixed(0)})`);
      }
    },
  });
  check((await path()) === 'WebRtc', 'the turn ended still on WebRTC');
  check(fallbacks.length === 0, 'WebRTC never dropped during the turn');
  await page.screenshot({ path: screenshot });
  log(`screenshot ${screenshot}`);

  const reloaded = Date.now();
  await page.goto(joinUrl.replace(/#join=.*/, ''));
  await waitFor('header.status[data-state="Live"]', 60_000);
  log(`Live ${Date.now() - reloaded}ms after reload without the fragment (reconnect)`);
  await page.locator('.turn-end').last().waitFor({ timeout: 10_000 });
  check((await page.locator('.reply').count()) >= 1, 'the transcript replayed after reload');
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
  await turn('are you there?');
  check((await path()) === 'Relayed', 'the turn completed on the relay');
  const failed = page.locator('header.status[data-path="Relayed"]:not([data-upgrade-error=""])');
  await failed.waitFor({ timeout: 20_000 });
  log(`upgrade error shown to the client: "${await status.getAttribute('data-upgrade-error')}"`);
  check((await status.getAttribute('data-state')) === 'Live', 'still Live after the upgrade failed');
  await page.screenshot({ path: screenshot });
} else if (scenario === 'drop') {
  await waitPath('WebRtc', UPGRADE_WITHIN_MS);
  log(`WebRtc ${await upgradeMs()}ms after Live on the relay (page clock)`);
  let droppedAt;
  await turn('drop me halfway', {
    during: async () => {
      await waitPath('Relayed', 10_000);
      droppedAt = Date.now();
      log(`fell back to the relay mid-turn: "${await status.getAttribute('data-upgrade-error')}"`);
    },
  });
  check(fallbacks.length === 1, 'the client fell back exactly once');
  await waitPath('WebRtc', 30_000);
  log(`WebRtc again ${Date.now() - droppedAt}ms after the drop`);
  await page.screenshot({ path: screenshot });
} else {
  throw new Error(`unknown scenario ${scenario}`);
}

check(linkErrors.length === 0, `the link never reported an error${linkErrors.length ? `: ${linkErrors.join(' | ')}` : ''}`);
check(consoleErrors.length === 0, `no console errors${consoleErrors.length ? `: ${consoleErrors.join(' | ')}` : ''}`);
await browser.close();
log('PASS');
