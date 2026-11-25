import type { Choosable, Draft } from "@conductor/core/session/start";
import type {
  Command,
  CommandResult,
  Effort,
  Models,
  Project,
  SessionId,
} from "@conductor/core/schemas/index";
import { choosable, effortsOf, optional, startOf, startable } from "@conductor/core/session/start";
import type { Host } from "@conductor/core/fold/host";
import { directories } from "@conductor/core/fold/host";

interface Fields {
  readonly directory: boolean;
  readonly efforts: ReadonlyArray<Effort>;
  readonly instances: ReadonlyArray<Choosable>;
  readonly modelDefault: string;
  readonly models: Models | undefined;
  readonly projects: ReadonlyArray<Project>;
  readonly suggested: ReadonlyArray<string>;
}

const fields = (host: Host, draft: Draft): Fields => {
  const instances = choosable(host);
  const models = instances.find((instance) => instance.key === draft.instance)?.models;
  return {
    directory: draft.project === "",
    efforts: models === undefined ? [] : effortsOf(models, optional(draft.model)),
    instances,
    modelDefault:
      typeof models?.default === "string" ? `Default (${models.default})` : "The CLI's default",
    models,
    projects: startable(host),
    suggested: directories(host),
  };
};

const start = (host: Host | undefined, draft: Draft): Command | undefined =>
  host === undefined ? undefined : startOf(host, draft);

const startedSession = (result: CommandResult): SessionId | undefined =>
  "Session" in result && typeof result.Session === "object" && "Started" in result.Session
    ? result.Session.Started.session
    : undefined;

const startDisabled = (command: Command | undefined, pending: boolean): boolean =>
  command === undefined || pending;

export { fields, start, startDisabled, startedSession };
export type { Fields };
