// The approvals and questions a session waits on, above its composer, each answered in one tap.
import { type ReactElement, useState } from "react";
import { openRequests } from "../fold/offers.ts";
import type { Session } from "../fold/session.ts";
import type { Item } from "../fold/transcript.ts";
import type { SessionId } from "../schemas/index.ts";
import {
	type Asked,
	answer,
	answersOf,
	decisions,
	nothing,
	type Offer,
	type Picked,
	type Send,
	toggled,
} from "./commands.ts";
import { useSending } from "./sending.ts";
import { ToolDetail } from "./transcript.tsx";
import { toolName } from "./words.ts";

type Run = (body: Offer["body"]) => void;

const Offers = ({
	offers,
	run,
	disabled,
}: {
	readonly offers: readonly Offer[];
	readonly run: Run;
	readonly disabled: boolean;
}): ReactElement => (
	<div className="offers">
		{offers.map((offer) => (
			<button
				key={offer.label}
				className={offer.tone.toLowerCase()}
				disabled={disabled}
				onClick={(): void => run(offer.body)}
				type="button"
			>
				{offer.label}
			</button>
		))}
	</div>
);

type Tool = Extract<Item, { readonly _tag: "Tool" }>;
type Asking = Extract<Item, { readonly _tag: "Question" }>;

const ApprovalRequest = ({
	id,
	item,
	asked,
	run,
	disabled,
}: {
	readonly id: SessionId;
	readonly item: Tool;
	readonly asked: Asked;
	readonly run: Run;
	readonly disabled: boolean;
}): ReactElement => (
	<section className="request">
		<h2>Approve? {toolName(item.tool)}</h2>
		<ToolDetail tool={item.tool} />
		{asked.reason === null ? null : <p className="reason">{asked.reason}</p>}
		{asked.forSession === null ? null : (
			<p className="for-session">For the session: {asked.forSession}</p>
		)}
		<Offers offers={decisions(id, asked)} run={run} disabled={disabled} />
	</section>
);

const QuestionRequest = ({
	id,
	item,
	run,
	disabled,
}: {
	readonly id: SessionId;
	readonly item: Asking;
	readonly run: Run;
	readonly disabled: boolean;
}): ReactElement => {
	const [picks, setPicks] = useState<readonly Picked[]>(() =>
		item.questions.map(() => nothing),
	);
	const pick = (at: number, next: Picked): void =>
		setPicks(picks.map((one, index) => (index === at ? next : one)));
	return (
		<form
			className="request"
			onSubmit={(event): void => {
				event.preventDefault();
				run(answer(id, item.request, answersOf(picks)));
			}}
		>
			{item.questions.map((question, at) => {
				const picked = picks[at] ?? nothing;
				return (
					<fieldset key={question.header}>
						<legend>{question.header}</legend>
						<p>{question.text}</p>
						<div className="options">
							{question.options.map((option) => (
								<button
									aria-pressed={picked.labels.includes(option.label)}
									disabled={disabled}
									key={option.label}
									onClick={(): void =>
										pick(at, toggled(question, picked, option.label))
									}
									title={option.description}
									type="button"
								>
									{option.label}
								</button>
							))}
						</div>
						<input
							aria-label={`Another answer to ${question.header}`}
							disabled={disabled}
							onChange={(event): void =>
								pick(at, { ...picked, typed: event.target.value })
							}
							placeholder="Or type an answer"
							value={picked.typed}
						/>
					</fieldset>
				);
			})}
			<div className="offers">
				<button className="primary" disabled={disabled} type="submit">
					Answer
				</button>
			</div>
		</form>
	);
};

const Request = ({
	id,
	item,
	run,
	disabled,
}: {
	readonly id: SessionId;
	readonly item: Item;
	readonly run: Run;
	readonly disabled: boolean;
}): ReactElement | null => {
	if (item._tag === "Tool" && item.approval._tag === "Asked") {
		return (
			<ApprovalRequest
				asked={item.approval}
				disabled={disabled}
				id={id}
				item={item}
				run={run}
			/>
		);
	}
	if (item._tag === "Question") {
		return (
			<QuestionRequest disabled={disabled} id={id} item={item} run={run} />
		);
	}
	return null;
};

const requestKey = (item: Item): string => {
	if (item._tag === "Question") {
		return `${item.request}`;
	}
	return item._tag === "Tool" && item.approval._tag === "Asked"
		? `${item.approval.request}`
		: item._tag;
};

const Requests = ({
	id,
	session,
	send,
}: {
	readonly id: SessionId;
	readonly session: Session;
	readonly send: Send;
}): ReactElement | null => {
	const sending = useSending();
	const open = openRequests(session);
	if (open.length === 0 && sending.notice === null) {
		return null;
	}
	return (
		<div className="requests">
			{sending.notice === null ? null : (
				<p className="notice">{sending.notice}</p>
			)}
			{open.map((item) => (
				<Request
					disabled={sending.pending}
					id={id}
					item={item}
					key={requestKey(item)}
					run={(body): void => sending.run(send, body)}
				/>
			))}
		</div>
	);
};

export { Offers, Requests };
