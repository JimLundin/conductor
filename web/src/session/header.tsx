import type { FunctionComponent } from "react";
import type { Session } from "@conductor/core/fold/session";
import { header } from "@conductor/core/view/session";

interface Props {
  readonly hostName: string;
  readonly session: Session;
}

const Header: FunctionComponent<Props> = ({ hostName, session }) => {
  const shown = header(session, hostName);
  return (
    <header className="session-bar">
      <h1 title={session.directory}>{session.directory}</h1>
      <p className="meta">
        {shown.meta.map((entry) => (
          <span key={entry.name}>{entry.text}</span>
        ))}
        <span className={`status ${shown.status.name}`}>{shown.status.text}</span>
      </p>
    </header>
  );
};

export { Header };
