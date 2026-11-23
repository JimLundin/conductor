// What a new session can start with on a host: its available instances, their models and
// efforts, and its projects and directories.
import type { Host } from "../fold/host.ts";
import type {
	CommandBody,
	Effort,
	InstanceId,
	ModelChoice,
	ModelId,
	Models,
	Project,
	StartAt,
} from "../proto/index.ts";

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
	host: "",
	instance: "",
	project: "",
	directory: "",
	model: "",
	effort: "",
} as const;

const instanceKey = (instance: InstanceId): string =>
	instance.account === "Own"
		? instance.provider
		: `${instance.provider}:${instance.account.Named}`;

const instanceLabel = (host: Host, instance: InstanceId): string => {
	const provider =
		host.catalog?.[instance.provider]?.label ?? instance.provider;
	return instance.account === "Own"
		? provider
		: `${provider} (${instance.account.Named})`;
};

// The instances a session can start on now: those whose CLI is available.
const choosable = (host: Host): readonly Choosable[] =>
	host.instances.flatMap(([instance, { availability }]) =>
		"Available" in availability
			? [
					{
						key: instanceKey(instance),
						instance,
						label: instanceLabel(host, instance),
						models: availability.Available.models,
					},
				]
			: [],
	);

// The model a session would run: the one chosen, or the instance's default.
const modelOf = (
	models: Models,
	model: ModelId | null,
): ModelChoice | undefined =>
	models.choices.find((choice) => choice.id === (model ?? models.default));

// The efforts the model takes, lowest first.
const effortsOf = (models: Models, model: ModelId | null): readonly Effort[] =>
	modelOf(models, model)?.efforts ?? [];

// A project starts in its first checkout on the host, so only a project checked out here can.
const startable = (host: Host): readonly Project[] =>
	host.projects.filter((project) => project.checkouts.length > 0);

const placeOf = (draft: Draft): StartAt | null => {
	if (draft.project !== "") {
		return { Project: draft.project };
	}
	const directory = draft.directory.trim();
	return directory === "" ? null : { Directory: directory };
};

const orNull = <T extends string>(value: string): T | null =>
	value === "" ? null : (value as T);

// The command that starts the session the draft describes on `host`, once it names an
// available instance and a place.
const startOf = (host: Host, draft: Draft): CommandBody | null => {
	const chosen = choosable(host).find((one) => one.key === draft.instance);
	const at = placeOf(draft);
	return chosen === undefined || at === null
		? null
		: {
				StartSession: {
					instance: chosen.instance,
					at,
					model: orNull<ModelId>(draft.model),
					effort: orNull<Effort>(draft.effort),
				},
			};
};

export {
	blank,
	type Choosable,
	choosable,
	type Draft,
	effortsOf,
	orNull,
	startable,
	startOf,
};
