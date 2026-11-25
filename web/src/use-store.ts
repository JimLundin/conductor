import type { PairedHost, View } from "@conductor/core/store";
import { createContext, useContext } from "react";
import { listed, sessionOf, sessionsOf } from "@conductor/core/store";
import type { HostKey } from "@conductor/core/saved";
import { Option } from "effect";
import type { Route } from "@conductor/core/route";
import type { Session } from "@conductor/core/fold/session";
import type { SessionId } from "@conductor/core/schemas/index";
import type { Store } from "#ui/store.ts";
import { useAtomValue } from "@effect/atom-react";

const StoreContext = createContext<Option.Option<Store>>(Option.none());

const useStore: () => Store = () => Option.getOrThrow(useContext(StoreContext));

const useView: () => View = () => useAtomValue(useStore().viewAtom);

const useHosts: () => ReadonlyArray<PairedHost> = () => listed(useView());

const useHost: (host: HostKey) => PairedHost | undefined = (host) => useView().hosts.get(host);

const useSessions: (host: HostKey) => ReadonlyArray<SessionId> = (host) =>
  sessionsOf(useView(), host);

const useSession: (host: HostKey, session: SessionId) => Session | undefined = (host, session) =>
  sessionOf(useView(), host, session);

const useRoute: () => Route = () => useView().route;

export { StoreContext, useHost, useHosts, useRoute, useSession, useSessions, useStore, useView };
