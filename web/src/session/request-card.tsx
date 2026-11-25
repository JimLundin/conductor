import type { Command, SessionId } from "@conductor/core/schemas/index";
import { ApprovalRequest } from "#ui/session/approval-request.tsx";
import type { FunctionComponent } from "react";
import { QuestionRequest } from "#ui/session/question-request.tsx";
import type { Request } from "@conductor/core/view/requests";

interface Props {
  readonly disabled: boolean;
  readonly id: SessionId;
  readonly request: Request;
  readonly run: (body: Command) => void;
}

const RequestCard: FunctionComponent<Props> = ({ disabled, id, request, run }) =>
  request.kind === "Approval" ? (
    <ApprovalRequest
      asked={request.asked}
      disabled={disabled}
      id={id}
      item={request.item}
      run={run}
    />
  ) : (
    <QuestionRequest disabled={disabled} id={id} item={request.item} run={run} />
  );

export { RequestCard };
