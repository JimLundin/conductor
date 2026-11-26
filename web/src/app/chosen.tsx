import type { ReactElement } from "react";

const choose = "Choose a session, project or issue, or start a new session.";
const Chosen = (): ReactElement => (
  <main className="pane pane-empty">
    <p>{choose}</p>
  </main>
);

export { Chosen };
