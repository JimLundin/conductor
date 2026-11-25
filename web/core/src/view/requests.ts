import { Approval, Item } from "@conductor/core/fold/transcript";
import type { Asked, Picked } from "@conductor/core/session/commands";
import type { Question } from "@conductor/core/schemas/index";
import type { Session } from "@conductor/core/fold/session";
import { nothing } from "@conductor/core/session/commands";
import { openRequests } from "@conductor/core/fold/offers";

type Tool = Extract<Item, { readonly _tag: "Tool" }>;
type Asking = Extract<Item, { readonly _tag: "Question" }>;
type Request =
  | { readonly kind: "Approval"; readonly key: string; readonly item: Tool; readonly asked: Asked }
  | { readonly kind: "Question"; readonly key: string; readonly item: Asking };

const none = 0;

const shownRequest = (item: Item): ReadonlyArray<Request> => {
  if (Item.$is("Tool")(item) && Approval.$is("Asked")(item.approval)) {
    return [{ asked: item.approval, item, key: String(item.approval.request), kind: "Approval" }];
  }
  return Item.$is("Question")(item) ? [{ item, key: String(item.request), kind: "Question" }] : [];
};

const requests = (session: Session): ReadonlyArray<Request> =>
  openRequests(session).flatMap((item) => shownRequest(item));
const emptyPicks = (item: Asking): ReadonlyArray<Picked> => item.questions.map(() => nothing);
const replacePick = (
  picks: ReadonlyArray<Picked>,
  at: number,
  next: Picked,
): ReadonlyArray<Picked> => picks.map((pick, index) => (index === at ? next : pick));
interface QuestionField {
  readonly index: number;
  readonly picked: Picked;
  readonly question: Question;
}

const questions = (item: Asking, picks: ReadonlyArray<Picked>): ReadonlyArray<QuestionField> =>
  item.questions.map((question, index) => ({ index, picked: picks[index] ?? nothing, question }));
const hiddenRequests = (count: number, notice: string | undefined): boolean =>
  count === none && typeof notice !== "string";

export { emptyPicks, hiddenRequests, questions, replacePick, requests };
export type { Asking, Request, Tool };
