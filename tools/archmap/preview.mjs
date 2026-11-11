// Screenshots the architecture page at desktop and phone widths, light and
// dark, and fails when the page scrolls sideways.
// With a third directory it also saves each section on its own, for review.
// usage: node tools/archmap/preview.mjs <index.html> <out-dir> [sections-dir]
import { spawn } from "node:child_process";
import { writeFileSync, mkdtempSync, mkdirSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const [page, out, sections] = process.argv.slice(2).map((p) => resolve(p));
if (sections) mkdirSync(sections, { recursive: true });
mkdirSync(out, { recursive: true });
const browsers = `${process.env.HOME}/.cache/ms-playwright`;
const chrome = process.env.CHROME ?? readdirSync(browsers).filter((d) => d.startsWith("chromium-")).sort().map((d) => `${browsers}/${d}/chrome-linux64/chrome`).pop();
const proc = spawn(chrome, [
  "--headless=new", "--no-sandbox", "--disable-gpu", "--hide-scrollbars",
  "--remote-debugging-port=9334", `--user-data-dir=${mkdtempSync(join(tmpdir(), "archmap-"))}`,
  "about:blank",
], { stdio: "ignore" });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let target;
for (let i = 0; i < 50 && !target; i++) {
  await sleep(200);
  try {
    target = (await (await fetch("http://127.0.0.1:9334/json")).json()).find((t) => t.type === "page");
  } catch {}
}
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener("open", r));
let id = 0;
const pending = new Map();
ws.addEventListener("message", (e) => {
  const msg = JSON.parse(e.data);
  if (pending.has(msg.id)) { pending.get(msg.id)(msg); pending.delete(msg.id); }
});
const send = (method, params = {}) => new Promise((ok, fail) => {
  const n = ++id;
  pending.set(n, (m) => (m.error ? fail(new Error(JSON.stringify(m.error))) : ok(m.result)));
  ws.send(JSON.stringify({ id: n, method, params }));
});
const evaluate = async (expression) => (await send("Runtime.evaluate", { expression, returnByValue: true })).result.value;

let failed = false;
for (const [name, width, mobile] of [["desktop", 1280, false], ["mobile", 390, true]]) {
  for (const dark of [false, true]) {
    await send("Emulation.setDeviceMetricsOverride", { width, height: 900, deviceScaleFactor: 1, mobile });
    await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: dark ? "dark" : "light" }] });
    await send("Page.navigate", { url: `file://${page}` });
    let last = 0;
    for (let i = 0; i < 20; i++) {
      await sleep(300);
      const now = await evaluate("document.readyState === 'complete' ? document.documentElement.scrollHeight : 0");
      if (now && now === last) break;
      last = now;
    }
    const scrollWidth = await evaluate("document.documentElement.scrollWidth");
    if (scrollWidth > width) { failed = true; console.error(`${name}: page is ${scrollWidth}px wide, viewport ${width}px`); }
    const height = await evaluate("Math.ceil(document.documentElement.scrollHeight)");
    const shot = await send("Page.captureScreenshot", {
      format: "png", captureBeyondViewport: true, clip: { x: 0, y: 0, width, height, scale: 1 },
    });
    const file = join(out, `${name}${dark ? "-dark" : ""}.png`);
    writeFileSync(file, Buffer.from(shot.data, "base64"));
    console.log(file, width, height);
    if (!sections) continue;
    const boxes = await evaluate("[...document.querySelectorAll('main > header, main > section')].map((s) => { const r = s.getBoundingClientRect(); return [s.id || 'header', r.top + scrollY, r.height]; })");
    for (const [sid, y, h] of boxes) {
      const part = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: true, clip: { x: 0, y, width, height: Math.ceil(h), scale: 1 } });
      writeFileSync(join(sections, `${name}${dark ? "-dark" : ""}-${sid}.png`), Buffer.from(part.data, "base64"));
    }
  }
}
ws.close();
proc.kill();
process.exit(failed ? 1 : 0);
