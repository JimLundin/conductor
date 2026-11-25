import { App } from "#ui/app.tsx";
import { Option } from "effect";
import type { ReactElement } from "react";
import { RegistryContext } from "@effect/atom-react";
import type { Store } from "#ui/store.ts";
import { StoreContext } from "#ui/use-store.ts";

const Root: ({ store }: { readonly store: Store }) => ReactElement = ({ store }) => (
  <RegistryContext value={store.registry}>
    <StoreContext value={Option.some(store)}>
      <App />
    </StoreContext>
  </RegistryContext>
);

export { Root };
