// The new-session form: a host, one of its instances, a project or directory, a model and an
// effort.
import { type ReactElement, useState } from "react";
import { directories, type Host } from "../fold/host.ts";
import type { ModelId, SessionId } from "../proto/index.ts";
import type { Send } from "./commands.ts";
import { useSending } from "./sending.ts";
import {
	blank,
	type Choosable,
	choosable,
	type Draft,
	effortsOf,
	orNull,
	startable,
	startOf,
} from "./start.ts";

// A paired host, by the key its link is kept under.
interface Paired {
	readonly key: string;
	readonly host: Host;
	readonly send: Send;
}

const hostName = (paired: Paired): string => paired.host.name ?? paired.key;

const InstanceFields = ({
	host,
	chosen,
	draft,
	change,
}: {
	readonly host: Host;
	readonly chosen: Choosable | undefined;
	readonly draft: Draft;
	readonly change: (next: Partial<Draft>) => void;
}): ReactElement => {
	const projects = startable(host);
	const suggested = directories(host);
	const models = chosen?.models;
	return (
		<>
			<label>
				Agent
				<select
					onChange={(event): void =>
						change({ instance: event.target.value, model: "", effort: "" })
					}
					value={draft.instance}
				>
					<option value="">Choose an agent</option>
					{choosable(host).map((one) => (
						<option key={one.key} value={one.key}>
							{one.label}
						</option>
					))}
				</select>
			</label>
			<label>
				Project
				<select
					onChange={(event): void => change({ project: event.target.value })}
					value={draft.project}
				>
					<option value="">A directory instead</option>
					{projects.map((project) => (
						<option key={project.id} value={project.id}>
							{project.title}
						</option>
					))}
				</select>
			</label>
			{draft.project === "" ? (
				<label>
					Directory
					<input
						list="directories"
						onChange={(event): void =>
							change({ directory: event.target.value })
						}
						placeholder="A directory on the host"
						value={draft.directory}
					/>
					<datalist id="directories">
						{suggested.map((path) => (
							<option key={path} value={path} />
						))}
					</datalist>
				</label>
			) : null}
			{models === undefined ? null : (
				<>
					<label>
						Model
						<select
							onChange={(event): void =>
								change({ model: event.target.value, effort: "" })
							}
							value={draft.model}
						>
							<option value="">
								{models.default === null
									? "The CLI's default"
									: `Default (${models.default})`}
							</option>
							{models.choices.map((choice) => (
								<option key={choice.id} value={choice.id}>
									{choice.name} · {choice.description}
								</option>
							))}
						</select>
					</label>
					<label>
						Effort
						<select
							onChange={(event): void => change({ effort: event.target.value })}
							value={draft.effort}
						>
							<option value="">The model's default</option>
							{effortsOf(models, orNull<ModelId>(draft.model)).map((effort) => (
								<option key={effort} value={effort}>
									{effort}
								</option>
							))}
						</select>
					</label>
				</>
			)}
		</>
	);
};

export const NewSession = ({
	hosts,
	started,
}: {
	readonly hosts: readonly Paired[];
	// The session the host started, to open it.
	readonly started: (host: string, session: SessionId) => void;
}): ReactElement => {
	const [draft, setDraft] = useState<Draft>({
		...blank,
		host: hosts.length === 1 ? (hosts[0]?.key ?? "") : "",
	});
	const paired = hosts.find((one) => one.key === draft.host);
	const sending = useSending();
	const change = (next: Partial<Draft>): void =>
		setDraft({ ...draft, ...next });
	const chosen =
		paired === undefined
			? undefined
			: choosable(paired.host).find((one) => one.key === draft.instance);
	const start = paired === undefined ? null : startOf(paired.host, draft);
	return (
		<form
			className="new-session"
			onSubmit={(event): void => {
				event.preventDefault();
				if (paired !== undefined && start !== null) {
					sending.run(paired.send, start, (result) => {
						if (typeof result === "object" && "Started" in result) {
							started(paired.key, result.Started.session);
						}
					});
				}
			}}
		>
			<h1>New session</h1>
			<label>
				Host
				<select
					onChange={(event): void =>
						setDraft({ ...blank, host: event.target.value })
					}
					value={draft.host}
				>
					<option value="">Choose a host</option>
					{hosts.map((one) => (
						<option key={one.key} value={one.key}>
							{hostName(one)}
						</option>
					))}
				</select>
			</label>
			{paired === undefined ? null : (
				<InstanceFields
					change={change}
					chosen={chosen}
					draft={draft}
					host={paired.host}
				/>
			)}
			{sending.notice === null ? null : (
				<p className="notice">{sending.notice}</p>
			)}
			<button
				className="primary"
				disabled={start === null || sending.pending}
				type="submit"
			>
				Start
			</button>
		</form>
	);
};

export type { Paired };
