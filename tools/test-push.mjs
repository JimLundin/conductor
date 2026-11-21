import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { runInNewContext } from "node:vm";

const script = await readFile(new URL("../crates/app/public/push.js", import.meta.url), "utf8");
const scope = "https://example.test/conductor/";
const host = "19".repeat(32);
const session = "3e".repeat(16);
const hash = `#session=${host}/${session}`;

function worker() {
  const handlers = new Map();
  const saved = new Map();
  const shown = [];
  const opened = [];
  const windows = [];

  runInNewContext(script, {
    URL,
    Response,
    Date: { now: () => 100_000 },
    caches: {
      open: async () => ({
        keys: async () => [...saved.keys()],
        match: async (url) => saved.get(String(url))?.clone(),
        put: async (url, response) => saved.set(String(url), response),
        delete: async (url) => saved.delete(String(url)),
      }),
    },
    self: {
      addEventListener: (name, handler) => handlers.set(name, handler),
      registration: {
        scope,
        showNotification: async (title, options) => shown.push({ title, options }),
      },
      clients: {
        matchAll: async () => windows,
        openWindow: async (url) => opened.push(url),
      },
    },
  });
  const send = async (name, event) => {
    let lifetime;
    handlers.get(name)({ ...event, waitUntil: (work) => { lifetime = work; } });
    await lifetime;
  };
  return { send, shown, opened, windows };
}

test("a permission denial sends no key request and registers no worker", async () => {
  const messages = [];
  const navigator = {
    serviceWorker: {
      addEventListener() {},
      register: () => assert.fail("permission denied"),
    },
  };
  const Notification = { requestPermission: async () => "denied" };
  const window = { Notification, PushManager: class {} };
  runInNewContext(script, { window, navigator, Notification, isSecureContext: true });
  await window.conductorPush.enable({
    send: (data) => messages.push(data.Err),
    recv: () => assert.fail("permission denied"),
  });
  assert.deepEqual(messages, ["Notification permission was not granted."]);
});

test("a worker shows one generic notification for fresh routing metadata", async () => {
  const { send, shown } = worker();
  const fresh = { host, session, event: "0000000000000010", expires: 200 };
  const push = (notice) => send("push", { data: { json: () => notice } });
  await Promise.all([push(fresh), push(fresh)]);
  await push({ ...fresh, event: "0000000000000009" });
  await push({ ...fresh, event: "0000000000000011", expires: 90 });
  assert.equal(shown.length, 1);
  assert.equal(shown[0].title, "Conductor");
  assert.equal(shown[0].options.body, "A session needs your attention.");
  assert.equal(shown[0].options.data.activate, hash);
});

test("activation opens the session or delivers it to the existing app", async () => {
  const { send, opened, windows } = worker();
  let closed = 0;
  const event = {
    notification: { close: () => { closed += 1; }, data: { activate: hash } },
  };
  await send("notificationclick", event);
  assert.deepEqual(opened, [scope + hash]);

  let focused = false;
  let activation;
  windows.push({
    url: scope,
    postMessage: (message) => { activation = message.activate; },
    focus: async () => { focused = true; },
  });
  await send("notificationclick", event);
  assert.equal(opened.length, 1);
  assert.equal(closed, 2);
  assert.equal(activation, hash);
  assert.equal(focused, true);
});

test("an activation posted just before the page watcher attaches is delivered", () => {
  const handlers = new Map();
  const workerHandlers = new Map();
  const location = { hash: "" };
  const navigator = {
    serviceWorker: {
      addEventListener: (name, handler) => workerHandlers.set(name, handler),
    },
  };
  const history = {
    replaceState: (_state, _title, next) => { location.hash = next; },
  };
  const window = {
    addEventListener: (name, handler) => handlers.set(name, handler),
    dispatchEvent: (event) => handlers.get(event.type)?.(),
  };
  class HashChangeEvent {
    constructor(type) { this.type = type; }
  }
  runInNewContext(script, { window, navigator, history, location, HashChangeEvent });

  workerHandlers.get("message")({ data: { activate: hash } });
  const activated = [];
  void window.conductorPush.watch({ send: (target) => activated.push(target) });
  assert.deepEqual(activated, [hash]);

  const next = `#session=${host}/${"4b".repeat(16)}`;
  workerHandlers.get("message")({ data: { activate: next } });
  assert.deepEqual(activated, [hash, next]);
});
