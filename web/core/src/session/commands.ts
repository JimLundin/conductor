import type {
  Answer,
  Command,
  CommandResult,
  Decision,
  Question,
  RequestId,
  SessionId,
  SessionRefusal,
} from "#core/schemas/index";
import type { Approval } from "#core/fold/transcript";
import { SendError } from "#core/link";
import type { Session } from "#core/fold/session";
import { controls } from "#core/fold/offers";
// The commands a session's screen sends, each offered as a button the view only renders.

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
  answers: ReadonlyArray<Answer>,
): Command => ({
  Session: { Answer: { answers, request, session } },
});

const resolve = (session: SessionId, request: RequestId, decision: Decision): Command => ({
  Session: { Resolve: { decision, request, session } },
});

// What a session offers besides its composer: stopping a turn, continuing a closed agent and
// Finishing on its issue, exactly as its controls allow.
const sessionOffers = (id: SessionId, session: Session): ReadonlyArray<Offer> => {
  const offered = controls(session);
  return [
    ...(offered.interrupt
      ? [
          {
            body: { Session: { Interrupt: { session: id } } },
            label: "Stop",
            tone: "Danger",
          } as const,
        ]
      : []),
    ...(offered.continue
      ? [
          {
            body: { Session: { Continue: { session: id } } },
            label: "Continue",
            tone: "Primary",
          } as const,
        ]
      : []),
    ...(offered.finish
      ? [
          {
            body: { Session: { Finish: { session: id } } },
            label: "Finish",
            tone: "Plain",
          } as const,
        ]
      : []),
  ];
};

// The one-tap answers to an approval: approving for the session only when the agent offers it.
const decisions = (id: SessionId, asked: Asked): ReadonlyArray<Offer> => [
  {
    body: resolve(id, asked.request, "Approve"),
    label: "Approve",
    tone: "Primary",
  },
  ...(asked.forSession === undefined
    ? []
    : [
        {
          body: resolve(id, asked.request, "ApproveForSession"),
          label: "Approve for session",
          tone: "Plain",
        } as const,
      ]),
  { body: resolve(id, asked.request, "Deny"), label: "Deny", tone: "Danger" },
];

// What the person picked for one question.
interface Picked {
  readonly labels: ReadonlyArray<string>;
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
const answersOf = (picks: ReadonlyArray<Picked>): ReadonlyArray<Answer> =>
  picks.map(({ labels, typed }) => (typed.trim() === "" ? labels : [...labels, typed.trim()]));

const firstLetter = 0;
const afterFirst = 1;

// Splits a variant's PascalCase name into a sentence.
const sentence = (name: string): string => {
  const words = name.replaceAll(/(?<lower>[a-z])(?<upper>[A-Z])/gu, "$1 $2");
  return `${words.charAt(firstLetter)}${words.slice(afterFirst).toLowerCase()}`;
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
// What follows.
const notice = (result: CommandResult): string | undefined =>
  "Session" in result && typeof result.Session === "object" && "Refused" in result.Session
    ? `The host refused: ${rejected(result.Session.Refused)}.`
    : undefined;

const failed = (error: SendError): string =>
  SendError.$match(error, {
    NoResult: () => "The host gave no result.",
    NotConnected: () => "Not connected to the host.",
    Wire: ({ reason }) => `The command failed: ${reason}.`,
  });

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
  sessionOffers,
  toggled,
};
