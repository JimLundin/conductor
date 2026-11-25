import type {
  Command,
  InstanceId,
  ModelChoice,
  ModelId,
  Models,
  Project,
  StartAt,
} from "#core/schemas/index";
import { Option, Schema } from "effect";
import { Effort } from "#core/schemas/index";
import type { Host } from "#core/fold/host";
// What a new session can start with on a host: its available instances, their models and
// Efforts, and its projects and directories.

interface Choosable {
  readonly key: string;
  readonly instance: InstanceId;
  readonly label: string;
  readonly models: Models;
}

// What the form holds; the empty string is "not chosen", which a select can show.
interface Draft {
  readonly host: string;
  readonly instance: string;
  readonly project: string;
  readonly directory: string;
  // Empty takes the instance's default model, or the model's default effort.
  readonly model: string;
  readonly effort: string;
}

const blank: Draft = {
  directory: "",
  effort: "",
  host: "",
  instance: "",
  model: "",
  project: "",
} as const;

const instanceKey = (instance: InstanceId): string =>
  instance.account === "Own" ? instance.provider : `${instance.provider}:${instance.account.Named}`;

const instanceLabel = (host: Host, instance: InstanceId): string => {
  const provider = host.catalog?.[instance.provider]?.label ?? instance.provider;
  return instance.account === "Own" ? provider : `${provider} (${instance.account.Named})`;
};

// The instances a session can start on now: those whose CLI is available.
const choosable = (host: Host): ReadonlyArray<Choosable> =>
  host.instances.flatMap(([instance, { availability }]) =>
    "Available" in availability
      ? [
          {
            instance,
            key: instanceKey(instance),
            label: instanceLabel(host, instance),
            models: availability.Available.models,
          },
        ]
      : [],
  );

// The model a session would run: the one chosen, or the instance's default.
const modelOf = (models: Models, model: ModelId | undefined): ModelChoice | undefined =>
  models.choices.find((choice) => choice.id === (model ?? models.default));

// The efforts the model takes, lowest first.
const effortsOf = (models: Models, model: ModelId | undefined): ReadonlyArray<Effort> =>
  modelOf(models, model)?.efforts ?? [];

// A project starts in its first checkout on the host, so only a project checked out here can.
const firstCheckout = 0;

const startable = (host: Host): ReadonlyArray<Project> =>
  host.projects.filter((project) => project.checkouts.at(firstCheckout) !== undefined);

const placeOf = (draft: Draft): StartAt | undefined => {
  if (draft.project !== "") {
    return { Project: draft.project };
  }
  const directory = draft.directory.trim();
  return directory === "" ? undefined : { Directory: directory };
};

const optional = (value: string): string | undefined => (value === "" ? undefined : value);

// The command that starts the session the draft describes on `host`, once it names an
// Available instance and a place.
const startOf = (host: Host, draft: Draft): Command | undefined => {
  const chosen = choosable(host).find((one) => one.key === draft.instance);
  const at = placeOf(draft);
  return chosen === undefined || at === undefined
    ? undefined
    : {
        Session: {
          Start: {
            at,
            effort: Option.getOrNull(Schema.decodeUnknownOption(Effort)(draft.effort)),
            instance: chosen.instance,
            model: Option.getOrNull(Option.fromUndefinedOr(optional(draft.model))),
          },
        },
      };
};

export { blank, type Choosable, choosable, type Draft, effortsOf, optional, startable, startOf };
