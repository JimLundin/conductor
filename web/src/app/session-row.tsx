import { useRoute, useSession } from "#ui/use-store.ts";
import type { FunctionComponent } from "react";
import type { HostKey } from "@conductor/core/saved";
import type { SessionId } from "@conductor/core/schemas/index";
import { sessionRow } from "@conductor/core/view/app";

interface Props {
  readonly host: HostKey;
  readonly id: SessionId;
}

const SessionRow: FunctionComponent<Props> = ({ host, id }) => {
  const session = useSession(host, id);
  const route = useRoute();
  const shown = session === undefined ? undefined : sessionRow(session, route, { host, id });
  return shown === undefined ? undefined : (
    <li>
      <a aria-current={shown.current} className="session-row" href={shown.href}>
        <span className="session-title">{shown.title}</span>
        <span className="session-status" data-status={shown.status}>
          {shown.text}
        </span>
      </a>
    </li>
  );
};

export { SessionRow };
