import { Result } from "effect";
import { fragmentOf, Route } from "@conductor/core/route";
import type { Ticket } from "@conductor/core/schemas/index";
import type { TicketError } from "@conductor/core/ticket";

const replacedFragment = (parsed: Result.Result<Ticket, TicketError>): string =>
  fragmentOf(Result.isSuccess(parsed) ? Route.Sessions() : Route.Pairing());

const joinNotice = (parsed: Result.Result<Ticket, TicketError>): TicketError | undefined =>
  Result.isFailure(parsed) ? parsed.failure : undefined;

export { joinNotice, replacedFragment };
