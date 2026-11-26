import type { Choosable, Draft } from "#core/session/start";
import type { CommandResult, Effort, Models, SessionId } from "#core/schemas/index";
import { directories } from "#core/fold/host";
import { effortsOf } from "#core/session/start";

const single = 1;
const first = 0;

const selection = (options: ReadonlyArray<Choosable>, key: string): Choosable | undefined =>
  options.length === single && key === "" ? options[first] : options.find((one) => one.key === key);

interface Fields {
  readonly efforts: ReadonlyArray<Effort>;
  readonly models: Models;
  readonly modelDefault: string;
}

const fields = (chosen: Choosable, draft: Draft): Fields => ({
  efforts: effortsOf(chosen.models, draft.model),
  modelDefault:
    typeof chosen.models.default === "string"
      ? `Default (${chosen.models.default})`
      : "The CLI's default",
  models: chosen.models,
});

const suggestions = (chosen: Choosable | undefined): ReadonlyArray<string> =>
  chosen === undefined ? [] : directories(chosen.host.fold);

const startedSession = (result: CommandResult): SessionId | undefined =>
  "Session" in result && typeof result.Session === "object" && "Started" in result.Session
    ? result.Session.Started.session
    : undefined;

export { fields, selection, startedSession, suggestions };
export type { Fields };
