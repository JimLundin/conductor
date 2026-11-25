import type { ReactElement } from "react";
import { sessionsHref } from "@conductor/core/view/app";

const sessions = "Sessions";

const Back = (): ReactElement => (
  <a className="back" href={sessionsHref}>
    {sessions}
  </a>
);

export { Back };
