import { Approval, Item } from "@conductor/core/fold/transcript";
import { Array, Option } from "effect";
import type { Asked, Offer } from "@conductor/core/session/commands";
import type { Command } from "@conductor/core/schemas/index";
import type { Host } from "@conductor/core/fold/host";
import { openRequests } from "@conductor/core/fold/offers";
import { session } from "#ui/test/session-events.ts";
import { sessionOf } from "#ui/test/examples.ts";

const offered = (offers: ReadonlyArray<Offer>, label: string): Command => {
  const chosen = Array.findFirst(offers, (offer) => offer.label === label);
  return Option.getOrThrow(Option.map(chosen, (offer) => offer.body));
};

const askedOf = (host: Host): Asked => {
  const found = sessionOf(host, session);
  const tools = openRequests(found).filter(Item.$is("Tool"));
  const approvals = tools.map((item) => item.approval);
  return Option.getOrThrow(Array.findFirst(approvals, Approval.$is("Asked")));
};

export { askedOf, offered };
