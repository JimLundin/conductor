import type { Command, Effort, InstanceId, Models, StartAt } from "#core/schemas/index";
import { Option } from "effect";
import type { PairedHost } from "#core/store";
import { hasCheckout } from "#core/session/work";
import { nameOf } from "#core/store";

interface Choosable {
  readonly key: string;
  readonly host: PairedHost;
  readonly instance: InstanceId;
  readonly label: string;
  readonly models: Models;
}

interface Draft {
  readonly agent: string;
  readonly project: string;
  readonly issue: string;
  readonly directory: string;
  readonly model: string;
  readonly effort: string;
}

const blank: Draft = {
  agent: "",
  directory: "",
  effort: "",
  issue: "",
  model: "",
  project: "",
} as const;

const agentLabel = (provider: string, instance: InstanceId): string =>
  instance.account === "Own" ? provider : `${provider} (${instance.account.Named})`;

const keyOf = (host: string, instance: InstanceId): string =>
  JSON.stringify([host, instance.provider, instance.account]);

const optionsOn = (host: PairedHost, issue: string): ReadonlyArray<Choosable> => {
  const taken = new Set(
    host.fold.sessions
      .values()
      .filter((session) => session.issue === issue)
      .map((session) => keyOf(host.key, session.instance)),
  );
  return host.fold.instances.flatMap(([instance, { availability }]) => {
    const key = keyOf(host.key, instance);
    if (!("Available" in availability) || taken.has(key)) {
      return [];
    }
    const provider = host.fold.catalog?.[instance.provider]?.label ?? instance.provider;
    return [
      {
        host,
        instance,
        key,
        label: `${agentLabel(provider, instance)} on ${nameOf(host)}`,
        models: availability.Available.models,
      },
    ];
  });
};

const distinguishHost = (option: Choosable, options: ReadonlyArray<Choosable>): Choosable =>
  options.some((other) => other.host.key !== option.host.key && other.label === option.label)
    ? { ...option, label: `${option.label} (${option.host.key})` }
    : option;

const eligible = (
  hosts: ReadonlyArray<PairedHost>,
  draft: Draft,
  directoryHost?: string,
): ReadonlyArray<Choosable> => {
  const options = hosts
    .filter(
      (host) =>
        host.status.state === "Live" &&
        (draft.project === ""
          ? directoryHost === undefined || host.key === directoryHost
          : hasCheckout(host.fold, draft.project, draft.issue)),
    )
    .flatMap((host) => optionsOn(host, draft.issue));
  return options.map((option) => distinguishHost(option, options));
};

const effortsOf = (models: Models, model: string): ReadonlyArray<Effort> =>
  models.choices.find((choice) => choice.id === (model || models.default))?.efforts ?? [];

const placeOf = (draft: Draft): StartAt | undefined => {
  if (draft.project !== "") {
    return draft.issue === "" ? { Project: draft.project } : { Issue: draft.issue };
  }
  const directory = draft.directory.trim();
  return directory === "" ? undefined : { Directory: directory };
};

const startOf = (chosen: Choosable | undefined, draft: Draft): Command | undefined => {
  const at = placeOf(draft);
  if (chosen === undefined || at === undefined) {
    return undefined;
  }
  const model = chosen.models.choices.find((one) => one.id === draft.model)?.id;
  const effort = effortsOf(chosen.models, model ?? "").find((one) => one === draft.effort);
  return {
    Session: {
      Start: {
        at,
        effort: Option.getOrNull(Option.fromUndefinedOr(effort)),
        instance: chosen.instance,
        model: Option.getOrNull(Option.fromUndefinedOr(model)),
      },
    },
  };
};

export { agentLabel, blank, type Choosable, type Draft, effortsOf, eligible, startOf };
