// A session's screen: its header, its live transcript, what it waits on, and its composer.
import { type ReactElement, useState } from "react";
import { controls, emptyTranscript, isWorking } from "../fold/offers.ts";
import type { Session } from "../fold/session.ts";
import type { SessionId } from "../proto/index.ts";
import { message, type Send, sessionOffers } from "./commands.ts";
import { Offers, Requests } from "./requests.tsx";
import { useSending } from "./sending.ts";
import { Items } from "./transcript.tsx";
import { closeText } from "./words.ts";

const statusText = (session: Session): string =>
	session.status._tag === "Closed"
		? `Closed: it ${closeText(session.status.reason)}`
		: session.status._tag;

const emptyText = {
	Starting: "Starting the agent…",
	Ready: "Send a message to start.",
	Closed: "The agent closed before any message.",
} as const;

const Header = ({ session }: { readonly session: Session }): ReactElement => (
	<header className="session-bar">
		<h1 title={session.directory}>{session.directory}</h1>
		<p className="meta">
			<span>{session.instance.provider}</span>
			{session.model === null ? null : <span>{session.model}</span>}
			{session.effort === null ? null : <span>{session.effort}</span>}
			<span className={`status ${session.status._tag.toLowerCase()}`}>
				{statusText(session)}
			</span>
		</p>
	</header>
);

const Composer = ({
	id,
	session,
	send,
}: {
	readonly id: SessionId;
	readonly session: Session;
	readonly send: Send;
}): ReactElement => {
	const [text, setText] = useState("");
	const sending = useSending();
	const offered = controls(session);
	const isOpen = offered.composer === "Open" && !sending.pending;
	return (
		<form
			className="composer"
			onSubmit={(event): void => {
				event.preventDefault();
				if (text.trim() !== "") {
					sending.run(send, message(id, text), () => setText(""));
				}
			}}
		>
			{sending.notice === null ? null : (
				<p className="notice">{sending.notice}</p>
			)}
			<textarea
				aria-label="Message"
				disabled={offered.composer === "Closed"}
				onChange={(event): void => setText(event.target.value)}
				placeholder={
					offered.composer === "Busy"
						? "The agent is working…"
						: "Message the agent"
				}
				value={text}
			/>
			<div className="offers">
				<Offers
					disabled={sending.pending}
					offers={sessionOffers(id, session)}
					run={(body): void => sending.run(send, body)}
				/>
				<button className="primary" disabled={!isOpen} type="submit">
					Send
				</button>
			</div>
		</form>
	);
};

export const SessionView = ({
	id,
	session,
	send,
}: {
	readonly id: SessionId;
	readonly session: Session;
	readonly send: Send;
}): ReactElement => {
	const empty = emptyTranscript(session);
	return (
		<section className="session">
			<Header session={session} />
			<div className="transcript">
				{empty === null ? (
					<Items items={session.items} subagents={session.subagents} />
				) : (
					<p className="empty">{emptyText[empty]}</p>
				)}
				{isWorking(session) ? <p className="working">Working…</p> : null}
			</div>
			<footer className="dock">
				<Requests id={id} send={send} session={session} />
				<Composer id={id} send={send} session={session} />
			</footer>
		</section>
	);
};
