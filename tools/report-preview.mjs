// Screenshots report-html's page at desktop and mobile widths, light and dark,
// cropped to the first group and the history section.
// usage: node tools/report-preview.mjs <index.html> <out-dir>
import { spawn } from "node:child_process";
import { writeFileSync, mkdtempSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const [page, out] = process.argv.slice(2);
mkdirSync(out, { recursive: true });
// CHROME overrides the browser; by default the newest Playwright Chromium.
const chrome = process.env.CHROME ?? (await import("node:fs")).readdirSync(`${process.env.HOME}/.cache/ms-playwright`).filter((d) => d.startsWith("chromium-")).sort().map((d) => `${process.env.HOME}/.cache/ms-playwright/${d}/chrome-linux64/chrome`).pop();
const proc = spawn(chrome, [
  "--headless=new", "--no-sandbox", "--disable-gpu", "--hide-scrollbars",
  "--remote-debugging-port=9333", `--user-data-dir=${mkdtempSync(join(tmpdir(), "prev-"))}`,
  "about:blank",
], { stdio: "ignore" });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let target;
for (let i = 0; i < 50 && !target; i++) {
  await sleep(200);
  try {
    const list = await (await fetch("http://127.0.0.1:9333/json")).json();
    target = list.find((t) => t.type === "page");
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
const send = (method, params = {}) => new Promise((resolve, reject) => {
  const n = ++id;
  pending.set(n, (m) => (m.error ? reject(new Error(JSON.stringify(m.error))) : resolve(m.result)));
  ws.send(JSON.stringify({ id: n, method, params }));
});

for (const [name, width, mobile] of [["desktop", 1280, false], ["mobile", 390, true]]) {
  for (const dark of [false, true]) {
    await send("Emulation.setDeviceMetricsOverride", { width, height: 900, deviceScaleFactor: 1, mobile });
    await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: dark ? "dark" : "light" }] });
    await send("Page.navigate", { url: `file://${page}` });
    await sleep(1500);
    const { result } = await send("Runtime.evaluate", {
      expression: "Math.ceil(document.querySelector('#history').getBoundingClientRect().bottom + window.scrollY + 24)",
      returnByValue: true,
    });
    const shot = await send("Page.captureScreenshot", {
      format: "png", captureBeyondViewport: true,
      clip: { x: 0, y: 0, width, height: result.value, scale: 1 },
    });
    const file = join(out, `${name}${dark ? "-dark" : ""}.png`);
    writeFileSync(file, Buffer.from(shot.data, "base64"));
    console.log(file, width, result.value);
  }
}
ws.close();
proc.kill();
