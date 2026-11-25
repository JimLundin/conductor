import { Pairing } from "#ui/app/pairing.tsx";
import { Pane } from "#ui/app/pane.tsx";
import type { ReactElement } from "react";
import { Route } from "@conductor/core/route";
import { Sidebar } from "#ui/app/sidebar.tsx";
import { useRoute } from "#ui/use-store.ts";

const App = (): ReactElement => {
  const route = useRoute();
  if (Route.$is("Pairing")(route)) {
    return <Pairing />;
  }
  return (
    <div className="shell" data-top={Route.$is("Sessions")(route) ? "list" : "pane"}>
      <Sidebar />
      <Pane route={route} />
    </div>
  );
};

export { App };
