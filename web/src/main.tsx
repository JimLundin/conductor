import { Effect } from "effect";
import { createRoot } from "react-dom/client";
import init, { connect } from "../web-iroh/web_iroh.js";
import { deviceName } from "./device.ts";
import { Root } from "./root.tsx";
import { makeStore, type Store } from "./store.ts";

const boundary = {
  pipe: { connect },
  client: deviceName(navigator.userAgent, navigator.maxTouchPoints),
  storage: localStorage,
  fragment: (): string => location.hash,
  go: (fragment: string): void => {
    location.hash = fragment;
  },
  replace: (fragment: string): void => {
    history.replaceState(null, "", fragment);
  },
} as const;

// The page's events the store follows: the fragment, and a network change or a return to the
// app, after which a connection may be dead without knowing it.
const listen = (store: Store): void => {
  addEventListener("hashchange", store.follow);
  addEventListener("online", store.reconnect);
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") {
      store.reconnect();
    }
  });
};

const element = document.querySelector("#root");
if (element) {
  await init();
  Effect.runFork(
    Effect.scoped(
      makeStore(boundary).pipe(
        Effect.tap((store) =>
          Effect.sync(() => {
            listen(store);
            createRoot(element).render(<Root store={store} />);
          }),
        ),
        Effect.andThen(Effect.never),
      ),
    ),
  );
}
