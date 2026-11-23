// The commands a session's screen sends, each offered as a button the view only renders.
import type { SendError } from "../link/command.ts";
import type { HostLink } from "../link/host.ts";
import { controls } from "../fold/offers.ts";
import type { Session } from "../fold/session.ts";
import type { Approval } from "../fold/transcript.ts";
import type {
	Answer,
	Command,
	CommandResult,
	Decision,
	Question,
	RequestId,
	SessionId,
	SessionRefusal,
} from "../proto/index.ts";

type Send = HostLink["send"];

type Tone = "Primary" | "Plain" | "Danger";

interface Offer {
	readonly label: string;
	readonly tone: Tone;
	readonly body: Command;
}

type Asked = Extract<Approval, { readonly _tag: "Asked" }>;

const message = (session: SessionId, text: string): Command => ({
	Session: { SendMessage: { session, text } },
});

const answer = (
	session: SessionId,
	request: RequestId,
	answers: readonly Answer[],
): Command => ({ Session: { Answer: { session, request, answers } } });

const resolve = (
	session: SessionId,
	request: RequestId,
	decision: Decision,
): Command => ({ Session: { Resolve: { session, request, decision } } });

// What a session offers besides its composer: stopping a turn, continuing a closed agent and
// finishing on its issue, exactly as its controls allow.
const sessionOffers = (id: SessionId, session: Session): readonly Offer[] => {
	const offered = controls(session);
	return [
		...(offered.interrupt
			? [
					{
						label: "Stop",
						tone: "Danger",
						body: { Session: { Interrupt: { session: id } } },
					} as const,
				]
			: []),
		...(offered.continue
			? [
					{
						label: "Continue",
						tone: "Primary",
						body: { Session: { Continue: { session: id } } },
					} as const,
				]
			: []),
		...(offered.finish
			? [
					{
						label: "Finish",
						tone: "Plain",
						body: { Session: { Finish: { session: id } } },
					} as const,
				]
			: []),
	];
};

// The one-tap answers to an approval: approving for the session only when the agent offers it.
const decisions = (id: SessionId, asked: Asked): readonly Offer[] => [
	{
		label: "Approve",
		tone: "Primary",
		body: resolve(id, asked.request, "Approve"),
	},
	...(asked.forSession === null
		? []
		: [
				{
					label: "Approve for session",
					tone: "Plain",
					body: resolve(id, asked.request, "ApproveForSession"),
				} as const,
			]),
	{ label: "Deny", tone: "Danger", body: resolve(id, asked.request, "Deny") },
];

// What the person picked for one question.
interface Picked {
	readonly labels: readonly string[];
	readonly typed: string;
}

const nothing: Picked = { labels: [], typed: "" } as const;

const toggled = (question: Question, picked: Picked, label: string): Picked => {
	if (question.selection === "One") {
		return { ...picked, labels: [label] };
	}
	return picked.labels.includes(label)
		? { ...picked, labels: picked.labels.filter((one) => one !== label) }
		: { ...picked, labels: [...picked.labels, label] };
};

// One answer per question in the order asked: the options chosen, then any text typed instead.
const answersOf = (picks: readonly Picked[]): readonly Answer[] =>
	picks.map(({ labels, typed }) =>
		typed.trim() === "" ? labels : [...labels, typed.trim()],
	);

// Splits a variant's PascalCase name into a sentence.
const sentence = (name: string): string => {
	const words = name.replaceAll(/(?<lower>[a-z])(?<upper>[A-Z])/gu, "$1 $2");
	return `${words.charAt(0)}${words.slice(1).toLowerCase()}`;
};

const rejected = (rejection: SessionRefusal): string => {
	if (typeof rejection === "string") {
		return sentence(rejection);
	}
	return Object.entries(rejection)
		.map(
			([name, detail]: readonly [string, object]) =>
				`${sentence(name)}: ${Object.values(detail).join(", ")}`,
		)
		.join("; ");
};

// What the person is told after a command: nothing once the host takes it, since its log shows
// what follows.
const notice = (result: CommandResult): string | null =>
	"Session" in result &&
	typeof result.Session === "object" &&
	"Refused" in result.Session
		? `The host refused: ${rejected(result.Session.Refused)}.`
		: null;

const failed = (error: SendError): string =>
	error._tag === "Wire"
		? `The command failed: ${error.reason}.`
		: {
				NotConnected: "Not connected to the host.",
				NoResult: "The host gave no result.",
			}[error._tag];

export {
	answer,
	answersOf,
	type Asked,
	decisions,
	failed,
	message,
	nothing,
	notice,
	type Offer,
	type Picked,
	type Send,
	sessionOffers,
	toggled,
};
