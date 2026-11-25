import type {
  Answer,
  CloseReason,
  Finished,
  Resolution,
  ToolKind,
  TurnOutcome,
} from "#core/schemas/index";
import { RequestState, ToolState } from "#core/fold/transcript";
import type { Subagent } from "#core/fold/subagent";
import { SubagentStatus } from "#core/fold/subagent";

const toolName = (tool: ToolKind): string => {
  if ("Command" in tool) {
    return "Ran";
  }
  if ("FileChange" in tool) {
    return "Changed files";
  }
  return tool.Other.name;
};

const toolStatus = (state: ToolState): string => {
  if (ToolState.$is("Running")(state)) {
    return "Running";
  }
  if (ToolState.$is("Stopped")(state)) {
    return "Stopped";
  }
  return state.outcome;
};

const answered = (answers: ReadonlyArray<Answer>): string =>
  answers.map((one) => one.join(", ")).join("; ");

const resolutionText = (resolution: Resolution): string => {
  if (resolution === "Withdrawn") {
    return "Withdrawn";
  }
  if ("Answered" in resolution) {
    return `Answered by ${resolution.Answered.by}: ${answered(resolution.Answered.answers)}`;
  }
  const { by, decision } = resolution.Decided;
  const decided = {
    Approve: "Approved",
    ApproveForSession: "Approved for the session",
    Deny: "Denied",
  }[decision];
  return `${decided} by ${by}`;
};

const requestText = (state: RequestState, open: string): string =>
  RequestState.$is("Open")(state) ? open : resolutionText(state.resolution);

const subagentStatus = (subagent: Subagent): string => {
  if (SubagentStatus.$is("Ended")(subagent.status)) {
    return subagent.status.outcome;
  }
  const { _tag: status } = subagent.status;
  return status;
};

const outcomeText = (outcome: TurnOutcome): string => {
  if (outcome === "Completed") {
    return "Turn completed";
  }
  if (outcome === "Interrupted") {
    return "Interrupted";
  }
  return `Turn failed: ${outcome.Failed.message}`;
};

const closeText = (reason: CloseReason): string => {
  if ("FailedToStart" in reason) {
    return `failed to start: ${reason.FailedToStart.message}`;
  }
  if ("Exited" in reason) {
    return `exited with ${reason.Exited.status}`;
  }
  return `stopped: ${reason.Stopped.reason}`;
};

const keptText = (
  kept: Extract<Finished, { readonly Landed: unknown }>["Landed"]["kept"],
): string => {
  if (typeof kept === "string") {
    return "kept locally";
  }
  return "Pushed" in kept
    ? `pushed to ${kept.Pushed.remote}`
    : `not pushed: ${kept.PushFailed.reason}`;
};

const finishedText = (finished: Finished): string => {
  if ("Failed" in finished) {
    return `Finishing failed: ${finished.Failed.reason}`;
  }
  const { commits, kept } = finished.Landed;
  return `Finished: ${commits} commits landed, ${keptText(kept)}`;
};

export { closeText, finishedText, outcomeText, requestText, subagentStatus, toolName, toolStatus };
