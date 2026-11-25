import { Approval } from "@conductor/core/fold/transcript";
import type { FunctionComponent } from "react";
import { requestText } from "@conductor/core/session/words";

const ApprovalLine: FunctionComponent<{ readonly approval: Approval }> = ({ approval }) =>
  Approval.$is("Asked")(approval) ? (
    <p className={`approval ${approval.state._tag.toLowerCase()}`}>
      {requestText(approval.state, "Waiting for approval")}
      {typeof approval.reason === "string" ? (
        <span className="reason">{` · ${approval.reason}`}</span>
      ) : undefined}
    </p>
  ) : undefined;

export { ApprovalLine };
