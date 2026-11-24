import type { ReactElement } from "react";
import { App } from "./app.tsx";
import type { Store } from "./store.ts";
import { StoreContext } from "./use-store.ts";

export const Root = ({ store }: { readonly store: Store }): ReactElement => (
  <StoreContext value={store}>
    <App />
  </StoreContext>
);
