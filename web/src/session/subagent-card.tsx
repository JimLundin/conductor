import type { FunctionComponent, ReactElement } from "react";
import type { Subagent } from "@conductor/core/fold/subagent";
import { SubagentStatus } from "@conductor/core/fold/subagent";
import { subagentStatus } from "@conductor/core/session/words";

interface Props {
  readonly children: Readonly<ReactElement>;
  readonly subagent: Subagent;
}

const SubagentCard: FunctionComponent<Props> = ({ children, subagent }) => (
  <details className="subagent" open={SubagentStatus.$is("Running")(subagent.status)}>
    <summary>
      <span className="title">{subagent.description}</span>
      {typeof subagent.kind === "string" ? (
        <span className="kind">{subagent.kind}</span>
      ) : undefined}
      <span className="status">{subagentStatus(subagent)}</span>
    </summary>
    {children}
  </details>
);

export { SubagentCard };
