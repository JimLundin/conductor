import { Chosen } from "#ui/app/chosen.tsx";
import type { FunctionComponent } from "react";
import { NewSessionPane } from "#ui/app/new-session-pane.tsx";
import { Pairing } from "#ui/app/pairing.tsx";
import { Route } from "@conductor/core/route";
import { SessionPane } from "#ui/app/session-pane.tsx";

const shown = Route.$match({
  NewSession: ({ host }) => <NewSessionPane host={host} />,
  Pairing: () => <Pairing />,
  Session: ({ host, session }) => <SessionPane host={host} session={session} />,
  Sessions: () => <Chosen />,
});

const Pane: FunctionComponent<{ readonly route: Route }> = ({ route }) => shown(route);

export { Pane };
