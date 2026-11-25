import type { FunctionComponent } from "react";
import type { Status } from "@conductor/core/link";
import { linkStates } from "@conductor/core/view/app";

const LinkStatus: FunctionComponent<{ readonly status: Status }> = ({ status }) => (
  <span className="link-status" data-state={status.state} title={status.error}>
    {linkStates[status.state]}
  </span>
);

export { LinkStatus };
