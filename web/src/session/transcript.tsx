// A session's transcript: its folded items in order, each tool call as a card with the
// sub-agents it started under it.
import type { ReactElement } from "react";
import type { Subagent } from "../fold/subagent.ts";
import {
	type Approval,
	Item,
	type RequestState,
	type ToolState,
} from "../fold/transcript.ts";
import type {
	Answer,
	CloseReason,
	Finished,
	Resolution,
	ToolKind,
	TurnOutcome,
} from "../proto/index.ts";
import { Markdown } from "./markdown.tsx";

type Tool = Extract<Item, { readonly _tag: "Tool" }>;

const toolName = (tool: ToolKind): string => {
	if ("Command" in tool) {
		return "Ran";
	}
	if ("FileChange" in tool) {
		return "Changed files";
	}
	return tool.Other.name;
};

const ToolDetail = ({ tool }: { readonly tool: ToolKind }): ReactElement => {
	if ("Command" in tool) {
		return <pre className="command">{tool.Command.command}</pre>;
	}
	if ("FileChange" in tool) {
		return (
			<ul className="files">
				{tool.FileChange.files.map((file) => (
					<li key={`${file.change} ${file.path}`}>
						<span className={`change ${file.change.toLowerCase()}`}>
							{file.change}
						</span>
						<code>{file.path}</code>
					</li>
				))}
			</ul>
		);
	}
	return <pre className="command">{tool.Other.detail}</pre>;
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

const ApprovalLine = ({
	approval,
}: {
	readonly approval: Approval;
}): ReactElement | null =>
	approval._tag === "Asked" ? (
		<p className={`approval ${approval.state._tag.toLowerCase()}`}>
			{requestText(approval.state, "Waiting for approval")}
			{approval.reason === null ? null : (
				<span className="reason"> · {approval.reason}</span>
			)}
		</p>
	) : null;

const subagentStatus = (subagent: Subagent): string => {
	if (subagent.status._tag === "Ended") {
		return subagent.status.outcome;
	}
	return subagent.status._tag;
};

const SubagentCard = ({
	subagent,
}: {
	readonly subagent: Subagent;
}): ReactElement => (
	<details
		className="subagent"
		open={subagent.status._tag === "Running"}
	>
		<summary>
			<span className="title">{subagent.description}</span>
			{subagent.kind === null ? null : (
				<span className="kind">{subagent.kind}</span>
			)}
			<span className="status">{subagentStatus(subagent)}</span>
		</summary>
		<Items items={subagent.items} subagents={[]} />
	</details>
);

const ToolCard = ({
	item,
	subagents,
}: {
	readonly item: Tool;
	readonly subagents: readonly Subagent[];
}): ReactElement => (
	<article
		className={`tool ${item.state._tag.toLowerCase()}`}
		data-status={toolStatus(item.state)}
	>
		<header>
			<span className="name">{toolName(item.tool)}</span>
			<span className="status">{toolStatus(item.state)}</span>
		</header>
		<ToolDetail tool={item.tool} />
		<ApprovalLine approval={item.approval} />
		{subagents
			.filter((subagent) => subagent.call === item.call)
			.map((subagent) => (
				<SubagentCard key={subagent.id} subagent={subagent} />
			))}
	</article>
);

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

const Marker = ({
	tone,
	text,
}: {
	readonly tone: string;
	readonly text: string;
}): ReactElement => <p className={`marker ${tone}`}>{text}</p>;

const ItemView = ({
	item,
	subagents,
}: {
	readonly item: Item;
	readonly subagents: readonly Subagent[];
}): ReactElement =>
	Item.$match(item, {
		User: ({ text, by }) => (
			<div className="user">
				<p className="bubble">{text}</p>
				<span className="by">{by}</span>
			</div>
		),
		Reply: ({ text, state }) => (
			<div className={`reply ${state.toLowerCase()}`}>
				<Markdown text={text} />
			</div>
		),
		Reasoning: ({ text, state }) => (
			<details className="reasoning" open={state === "Streaming"}>
				<summary>{state === "Streaming" ? "Thinking" : "Thought"}</summary>
				<Markdown text={text} />
			</details>
		),
		Tool: (tool) => <ToolCard item={tool} subagents={subagents} />,
		Question: ({ questions, state }) => (
			<article className={`question ${state._tag.toLowerCase()}`}>
				{questions.map((question) => (
					<p key={question.header}>
						<strong>{question.header}</strong> {question.text}
					</p>
				))}
				<p className="approval">
					{requestText(state, "Waiting for your answer")}
				</p>
			</article>
		),
		TurnCompleted: ({ outcome }) => (
			<Marker
				tone={outcome === "Completed" ? "done" : "stopped"}
				text={outcomeText(outcome)}
			/>
		),
		Continued: ({ reason }) => (
			<Marker tone="done" text={`Continued after it ${closeText(reason)}`} />
		),
		ModelChanged: ({ reason }) => (
			<Marker tone="note" text={`Model changed: ${reason}`} />
		),
		Finishing: () => <Marker tone="note" text="Finishing on its issue" />,
		Finished: ({ finished }) => (
			<Marker tone="done" text={finishedText(finished)} />
		),
		Error: ({ message }) => <Marker tone="error" text={message} />,
	});

// Items have no identity of their own: the transcript only appends and changes them in place,
// so an item's position is its identity.
const Items = ({
	items,
	subagents,
}: {
	readonly items: readonly Item[];
	readonly subagents: readonly Subagent[];
}): ReactElement => (
	<ol className="items">
		{items
			.map((item, at) => ({ item, key: `${at}` }))
			.map(({ item, key }) => (
				<li key={key} className={`item ${item._tag.toLowerCase()}`}>
					<ItemView item={item} subagents={subagents} />
				</li>
			))}
	</ol>
);

export { closeText, Items, resolutionText, toolName, ToolDetail };
