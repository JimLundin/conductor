(() => {
  const sessionHash = (notice) => `#session=${notice.host}/${notice.session}`;
  const validHash = (hash) => /^#session=[0-9a-f]{64}\/[0-9a-f]{32}$/.test(hash);
  const cacheName = "conductor-push-seen";

  if (typeof window !== "undefined") {
    if ("serviceWorker" in navigator) {
      navigator.serviceWorker.addEventListener("message", ({ data }) => {
        if (typeof data?.activate !== "string" || !validHash(data.activate)) return;
        history.replaceState(null, "", data.activate);
        window.dispatchEvent(new HashChangeEvent("hashchange"));
      });
    }
    window.conductorPush = {
      async watch(bridge) {
        const activate = () => {
          if (validHash(location.hash)) bridge.send(location.hash);
        };
        window.addEventListener("hashchange", activate);
        activate();
        await new Promise(() => {});
      },
      async enable(bridge) {
        if (!isSecureContext || !("serviceWorker" in navigator) ||
            !("Notification" in window) || !("PushManager" in window)) {
          bridge.send({ Err: "Push notifications are unavailable in this browser." });
          return null;
        }
        let permission;
        try {
          permission = await Notification.requestPermission();
        } catch {
          bridge.send({ Err: "This browser could not request notification permission." });
          return null;
        }
        if (permission !== "granted") {
          bridge.send({ Err: "Notification permission was not granted." });
          return null;
        }
        bridge.send({ Ok: null });
        const key = await bridge.recv();
        if (!key) return null;
        try {
          const registration = await navigator.serviceWorker.register("push.js");
          await navigator.serviceWorker.ready;
          const encoded = key.replaceAll("-", "+").replaceAll("_", "/");
          const applicationServerKey = Uint8Array.from(atob(encoded), char => char.charCodeAt(0));
          const existing = await registration.pushManager.getSubscription();
          if (existing && !equalKeys(existing.options.applicationServerKey, applicationServerKey)) {
            await existing.unsubscribe();
          }
          const subscription = await registration.pushManager.subscribe({
            userVisibleOnly: true,
            applicationServerKey,
          });
          const { endpoint, keys } = subscription.toJSON();
          return { Ok: { endpoint, p256dh: keys.p256dh, auth: keys.auth } };
        } catch {
          return { Err: "Could not subscribe this installation to notifications." };
        }
      },
    };

    function equalKeys(first, second) {
      const bytes = new Uint8Array(first);
      return bytes.length === second.length && bytes.every((byte, i) => byte === second[i]);
    }
    return;
  }

  let pending = Promise.resolve();

  self.addEventListener("push", (event) => {
    pending = pending.catch(() => {}).then(() => show(event));
    event.waitUntil(pending);
  });

  async function show(event) {
    let notice;
    try {
      notice = event.data?.json();
    } catch {
      return;
    }
    if (!notice || !validHash(sessionHash(notice)) || !/^[0-9a-f]{16}$/.test(notice.event)
        || !Number.isSafeInteger(notice.expires) || notice.expires <= Date.now() / 1000) return;

    const now = Date.now() / 1000;
    const cache = await caches.open(cacheName);
    const entries = await cache.keys();
    for (const entry of entries) {
      const cached = await cache.match(entry);
      if (Number(cached?.headers.get("expires-at")) <= now) await cache.delete(entry);
    }
    const target = new URL("push-seen", self.registration.scope);
    target.searchParams.set("session", `${notice.host}/${notice.session}`);
    const prior = await cache.match(target);
    if (prior && (await prior.text()) >= notice.event) return;

    await self.registration.showNotification("Conductor", {
      body: "A session needs your attention.",
      tag: `${notice.host}/${notice.session}`,
      data: { activate: sessionHash(notice) },
    });
    await cache.put(target, new Response(notice.event, {
      headers: { "expires-at": notice.expires.toString() },
    }));
  }

  self.addEventListener("notificationclick", (event) => {
    event.notification.close();
    const hash = event.notification.data?.activate;
    if (typeof hash === "string" && validHash(hash)) event.waitUntil(activate(hash));
  });

  async function activate(hash) {
    const target = new URL(self.registration.scope);
    const windows = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
    const app = windows.find(({ url }) => {
      const page = new URL(url);
      return page.origin === target.origin && page.pathname === target.pathname;
    });
    if (app) {
      app.postMessage({ activate: hash });
      await app.focus();
    } else {
      target.hash = hash;
      await self.clients.openWindow(target.href);
    }
  }
})();
