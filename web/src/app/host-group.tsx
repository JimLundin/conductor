import type { FunctionComponent } from "react";
import { LinkStatus } from "#ui/app/link-status.tsx";
import { Notice } from "#ui/session/notice.tsx";
import type { PairedHost } from "@conductor/core/store";
import { SessionRow } from "#ui/app/session-row.tsx";
import { nameOf } from "@conductor/core/store";
import { newSessionHref } from "@conductor/core/view/app";
import { useSessions } from "#ui/use-store.ts";

const start = "Start in directory";
const empty = "No sessions yet.";
const none = 0;

const HostGroup: FunctionComponent<{ readonly host: PairedHost }> = ({ host }) => {
  const sessions = useSessions(host.key);
  return (
    <section className="host-group" data-host={host.key}>
      <header className="host-header">
        <h2>{nameOf(host)}</h2>
        <LinkStatus status={host.status} />
        <a className="new-session" href={newSessionHref(host.key)}>
          {start}
        </a>
      </header>
      <Notice tone="link-error" text={host.status.error} />
      {sessions.length === none ? (
        <p className="empty">{empty}</p>
      ) : (
        <ul className="sessions">
          {sessions.map((session) => (
            <SessionRow host={host.key} id={session} key={session} />
          ))}
        </ul>
      )}
    </section>
  );
};

export { HostGroup };
