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

interface Choosable {
  readonly key: string;
  readonly instance: InstanceId;
  readonly label: string;
  readonly models: Models;
}

interface Draft {
  readonly host: string;
  readonly instance: string;
  readonly project: string;
  readonly directory: string;
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

const modelOf = (models: Models, model: ModelId | undefined): ModelChoice | undefined =>
  models.choices.find((choice) => choice.id === (model ?? models.default));

const effortsOf = (models: Models, model: ModelId | undefined): ReadonlyArray<Effort> =>
  modelOf(models, model)?.efforts ?? [];

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
