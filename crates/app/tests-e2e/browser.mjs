// Drives the web build in headless Chromium at an iPhone viewport against a
// live `conductord s3-host`, through n0's relays.
//
// usage: node browser.mjs <join-url> <screenshot.png>

import { chromium } from 'playwright';

const [joinUrl, screenshot] = process.argv.slice(2);
if (!joinUrl || !screenshot) {
  console.error('usage: node browser.mjs <join-url> <screenshot.png>');
  process.exit(2);
}

const t0 = Date.now();
const log = (line) => console.log(`[${((Date.now() - t0) / 1000).toFixed(3).padStart(7)}s] ${line}`);
const check = (ok, what) => {
  if (!ok) throw new Error(`FAILED: ${what}`);
  log(`ok: ${what}`);
};

const browser = await chromium.launch();
const context = await browser.newContext({
  viewport: { width: 390, height: 844 },
  deviceScaleFactor: 3,
  isMobile: true,
  hasTouch: true,
});
const page = await context.newPage();
const consoleErrors = [];
page.on('console', (msg) => {
  if (msg.type() === 'error') consoleErrors.push(msg.text());
});
page.on('pageerror', (err) => consoleErrors.push(String(err)));

const status = page.locator('header.status');
const waitLive = async () => {
  const start = Date.now();
  await page.locator('header.status[data-state="Live"]').waitFor({ timeout: 60_000 });
  return Date.now() - start;
};

log(`open ${joinUrl.replace(/#join=.*/, '#join=<ticket>')}`);
const opened = Date.now();
await page.goto(joinUrl);
await waitLive();
log(`Live ${Date.now() - opened}ms after navigation (first connect)`);
log(`status line: "${(await status.innerText()).replace(/\s+/g, ' ')}"`);
check(!page.url().includes('#join='), 'the join fragment was removed from the address bar');
check(
  (await page.evaluate(() => localStorage.getItem('conductor.ticket'))) !== null,
  'the ticket is in local storage',
);

const input = page.locator('.composer input');
await input.fill('hello from the phone');
await input.press('Enter');
log('submitted a prompt');

const reply = page.locator('.reply.streaming').last();
await reply.waitFor({ timeout: 20_000 });
const seen = new Set();
while (seen.size < 4 && (await reply.count()) > 0) {
  seen.add(await reply.innerText().catch(() => ''));
  await page.waitForTimeout(60);
}
check(seen.size >= 3, `deltas rendered live (${seen.size} distinct partial texts seen)`);

const approve = page.locator('.approval[data-open="true"] button.approve');
await approve.waitFor({ timeout: 20_000 });
log(`approval card: "${(await page.locator('.approval[data-open="true"]').innerText()).replace(/\s+/g, ' ')}"`);
for (const control of ['.approval button.approve', '.approval button.deny', '.composer input', '.composer button']) {
  const box = await page.locator(control).last().boundingBox();
  check(box.height >= 44 && box.width >= 44, `${control} is at least 44px (${box.width.toFixed(0)}x${box.height.toFixed(0)})`);
}
await approve.tap();
log('tapped Approve');

await page.locator('.turn-end').last().waitFor({ timeout: 20_000 });
log(`resolved: "${await page.locator('.approval .resolved').last().innerText()}"`);
log(`message: "${await page.locator('.message').last().innerText()}"`);
log('Turn finished rendered');
const path = await status.getAttribute('data-path');
log(`path reported by the client: ${path}`);
check(path === 'Relayed', 'the browser client is relayed');
await page.screenshot({ path: screenshot });
log(`screenshot ${screenshot}`);

const reloaded = Date.now();
await page.goto(joinUrl.replace(/#join=.*/, ''));
await waitLive();
log(`Live ${Date.now() - reloaded}ms after reload without the fragment (reconnect)`);
await page.locator('.turn-end').last().waitFor({ timeout: 10_000 });
check((await page.locator('.reply').count()) >= 1, 'the transcript replayed after reload');

check(consoleErrors.length === 0, `no console errors${consoleErrors.length ? `: ${consoleErrors.join(' | ')}` : ''}`);
await browser.close();
log('PASS');
