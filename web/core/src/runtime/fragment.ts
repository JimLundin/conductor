import { Route, fragmentOf } from "#core/route";
import type { TicketError } from "#core/ticket";

const replacedFragment: (error: TicketError | undefined) => string = (error) =>
  fragmentOf(error === undefined ? Route.Sessions() : Route.Pairing());

export { replacedFragment };
