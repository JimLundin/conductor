// The store as React reads it: each hook re-renders its component when the store changes.
import { createContext, useContext, useSyncExternalStore } from "react";
import type { Session } from "./fold/session.ts";
import type { SessionId } from "./schemas/index.ts";
import type { Route } from "./route.ts";
import type { HostKey } from "./saved.ts";
import { listed, type PairedHost, type Store, sessionOf, sessionsOf, type View } from "./store.ts";

export const StoreContext = createContext<Store | null>(null);

export const useStore = (): Store => {
  const store = useContext(StoreContext);
  if (store === null) {
    throw new Error("the app renders inside a StoreContext");
  }
  return store;
};

export const useView = (): View => {
  const store = useStore();
  return useSyncExternalStore(store.subscribe, store.view, store.view);
};

export const useHosts = (): readonly PairedHost[] => listed(useView());

export const useHost = (host: HostKey): PairedHost | undefined => useView().hosts.get(host);

export const useSessions = (host: HostKey): readonly SessionId[] => sessionsOf(useView(), host);

export const useSession = (host: HostKey, session: SessionId): Session | undefined =>
  sessionOf(useView(), host, session);

export const useRoute = (): Route => useView().route;
