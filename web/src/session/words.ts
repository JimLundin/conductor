// How the session screens word what the fold holds.
import type { Subagent } from "../fold/subagent.ts";
import type { RequestState, ToolState } from "../fold/transcript.ts";
import type {
	Answer,
	CloseReason,
	Finished,
	Resolution,
	ToolKind,
	TurnOutcome,
} from "../schemas/index.ts";

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
	if (state._tag === "Running") {
		return "Running";
	}
	if (state._tag === "Stopped") {
		return "Stopped";
	}
	return state.outcome;
};

const answered = (answers: readonly Answer[]): string =>
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
	state._tag === "Open" ? open : resolutionText(state.resolution);

const subagentStatus = (subagent: Subagent): string => {
	if (subagent.status._tag === "Ended") {
		return subagent.status.outcome;
	}
	return subagent.status._tag;
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

const finishedText = (finished: Finished): string => {
	if ("Failed" in finished) {
		return `Finishing failed: ${finished.Failed.reason}`;
	}
	const { commits, kept } = finished.Landed;
	const where =
		typeof kept === "string"
			? "kept locally"
			: "Pushed" in kept
				? `pushed to ${kept.Pushed.remote}`
				: `not pushed: ${kept.PushFailed.reason}`;
	return `Finished: ${commits} commits landed, ${where}`;
};

export {
	closeText,
	finishedText,
	outcomeText,
	requestText,
	subagentStatus,
	toolName,
	toolStatus,
};
