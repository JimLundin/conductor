import type { ReactElement } from "react";

const choose = "Choose a session, or start a new one on a host.";
const Chosen = (): ReactElement => (
  <main className="pane pane-empty">
    <p>{choose}</p>
  </main>
);

export { Chosen };
