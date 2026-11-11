// Screenshots the app's screens over the `#fixture` sessions, at desktop
// width and at 390x844, light and dark. Expects `dx build --platform web
// --release` in crates/app first.
// usage: node tools/app-screens.mjs <web-public-dir> <out-dir>
import { spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createRequire } from "node:module";

const [publicDir, out] = process.argv.slice(2).map((path) => resolve(path));
const require = createRequire(resolve("crates/app/tests-e2e/package.json"));
const { chromium } = require("playwright");

mkdirSync(out, { recursive: true });
const site = mkdtempSync(join(tmpdir(), "screens-"));
symlinkSync(publicDir, join(site, "conductor"));
const port = 8765;
const server = spawn("python3", ["-m", "http.server", String(port), "--bind", "127.0.0.1", "--directory", site], { stdio: "ignore" });
const url = `http://127.0.0.1:${port}/conductor/#fixture`;

const browser = await chromium.launch();
const shoot = async (page, name) => {
  await page.waitForTimeout(450);
  const file = join(out, `${name}.png`);
  await page.screenshot({ path: file });
  console.log(file);
};
const row = (page, title) => page.locator(".session-row", { has: page.locator(".row-title", { hasText: new RegExp(`^${title}$`) }) });

try {
  for (const scheme of ["light", "dark"]) {
    const desktop = await browser.newPage({ viewport: { width: 1280, height: 800 }, colorScheme: scheme, reducedMotion: "reduce" });
    for (let tries = 0; ; tries++) {
      try { await desktop.goto(url); break; } catch (error) { if (tries > 20) throw error; await desktop.waitForTimeout(250); }
    }
    await desktop.locator(".session-row").first().waitFor();
    await row(desktop, "conductor").click();
    await shoot(desktop, `desktop-${scheme}-running`);
    await row(desktop, "website").click();
    await shoot(desktop, `desktop-${scheme}-approval`);
    await row(desktop, "scratch").click();
    await shoot(desktop, `desktop-${scheme}-closed`);
    await desktop.getByRole("button", { name: "New session" }).click();
    await desktop.locator('input[name="directory"]').fill("/home/jim/src/conductr");
    await desktop.getByRole("button", { name: "Start session" }).click();
    await desktop.locator(".hint.error").waitFor();
    await shoot(desktop, `desktop-${scheme}-new-session`);
    await desktop.close();

    const phone = await browser.newPage({
      viewport: { width: 390, height: 844 }, deviceScaleFactor: 2, isMobile: true, hasTouch: true,
      colorScheme: scheme, reducedMotion: "reduce",
    });
    await phone.goto(url);
    await phone.locator(".session-row").first().waitFor();
    await shoot(phone, `phone-${scheme}-list`);
    for (const [title, name] of [["conductor", "running"], ["website", "approval"], ["notes", "idle"]]) {
      await row(phone, title).tap();
      await shoot(phone, `phone-${scheme}-${name}`);
      await phone.locator(".session-pane .back").tap();
    }
    await phone.getByRole("button", { name: "New session" }).tap();
    await phone.locator(".segment", { hasText: "Codex" }).tap();
    await phone.locator('input[name="directory"]').fill("/home/jim/src/website");
    await phone.getByRole("button", { name: "Start session" }).tap();
    await phone.locator(".hint.error").waitFor();
    await phone.locator('input[name="directory"]').blur();
    await shoot(phone, `phone-${scheme}-new-session`);
    await phone.close();
  }
} finally {
  await browser.close();
  server.kill();
}
