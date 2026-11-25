import type { Command, SessionId } from "@conductor/core/schemas/index";
import type { Asked } from "@conductor/core/session/commands";
import type { FunctionComponent } from "react";
import { Notice } from "#ui/session/notice.tsx";
import { Offers } from "#ui/session/offers.tsx";
import type { Tool } from "@conductor/core/view/requests";
import { ToolDetail } from "#ui/session/tool-detail.tsx";
import { decisions } from "@conductor/core/session/commands";
import { toolName } from "@conductor/core/session/words";

interface Props {
  readonly asked: Asked;
  readonly disabled: boolean;
  readonly id: SessionId;
  readonly item: Tool;
  readonly run: (body: Command) => void;
}

const ApprovalRequest: FunctionComponent<Props> = ({ asked, disabled, id, item, run }) => {
  const offers = decisions(id, asked);
  const forSession =
    typeof asked.forSession === "string" ? `For the session: ${asked.forSession}` : undefined;
  return (
    <section className="request">
      <h2>{`Approve? ${toolName(item.tool)}`}</h2>
      <ToolDetail tool={item.tool} />
      <Notice tone="reason" text={asked.reason} />
      <Notice tone="for-session" text={forSession} />
      <Offers disabled={disabled} offers={offers} run={run} />
    </section>
  );
};

export { ApprovalRequest };
