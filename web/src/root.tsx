import { type ReactElement, useSyncExternalStore } from "react";
import { App } from "./app.tsx";
import { route } from "./route.ts";

const subscribe = (changed: () => void): (() => void) => {
	globalThis.addEventListener("hashchange", changed);
	return (): void => globalThis.removeEventListener("hashchange", changed);
};

const fragment = (): string => globalThis.location.hash;

// The app at the page's URL fragment, following it as it changes.
export const Root = (): ReactElement => (
	<App route={route(useSyncExternalStore(subscribe, fragment))} />
);
