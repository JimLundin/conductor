import type { Choosable, Draft } from "@conductor/core/session/start";
import type { Instances, Issue, Observation, Project } from "@conductor/core/schemas/index";
import { blank, effortsOf, eligible, startOf } from "@conductor/core/session/start";
import { describe, expect, it } from "@effect/vitest";
import { directory, session, turnThrough } from "#ui/test/session-events.ts";
import { newSessionForm, pairedHost } from "#ui/test/new-session.tsx";
import { Option } from "effect";
import type { PairedHost } from "@conductor/core/store";
import { selection } from "@conductor/core/view/start";
import { sessionOf } from "#ui/test/examples.ts";

const projectId = "hyn162";
const issue: Issue = {
  body: "Keep the issue when choosing an agent.",
  branch: "issue/sessions",
  canvases: [],
  checklist: [],
  created: null,
  fields: {},
  id: "local~00000003",
  kind: "task",
  links: {},
  log: [],
  notes: [],
  project: projectId,
  sections: [],
  stage: null,
  status: "active",
  title: "Simplify sessions",
} as const;
const observed: Observation = {
  completeness: { fields: "Complete", notes: "Complete", relations: {} },
  findings: [],
  flow_findings: [],
  locator: null,
  observed_at: null,
  revision: null,
  state: { Valid: { body: issue.body, issue } },
} as const;
const project: Project = {
  checkouts: [{ origin: "Found", path: directory }],
  cloning: "Idle",
  id: projectId,
  issues: [observed],
  name: "conductor",
  profiles: [],
  provides: [],
  remote: null,
  title: "Conductor",
} as const;
const prepared = (name: string, source = project): PairedHost =>
  pairedHost(name, { ...turnThrough(0), projects: [source] });

const onIssue: Draft = { ...blank, issue: issue.id, project: projectId } as const;
const choice = (hosts: ReadonlyArray<PairedHost>, draft: Draft): Choosable | undefined =>
  selection(eligible(hosts, draft), draft.agent);

const namedInstances = (host: PairedHost): Instances =>
  host.fold.instances.flatMap(([instance, state]) =>
    "Available" in state.availability
      ? ([
          [instance, state],
          [{ ...instance, account: { Named: "work" } }, state],
        ] as const)
      : [],
  );

describe("starting a session", () => {
  it("offers the only eligible account for the selected issue", () => {
    expect.hasAssertions();
    const offline = {
      ...prepared("offline"),
      status: { error: undefined, state: "Disconnected" },
    } as const;
    const hosts = [
      offline,
      prepared("laptop", { ...project, checkouts: [] }),
      prepared("other", { ...project, issues: [] }),
      prepared("devbox"),
    ];
    const form = newSessionForm({ hosts, issue: issue.id, project: projectId });
    expect(form).toContain(issue.body);
    expect(form).toContain('type="submit">Start with Claude Code on devbox</button>');
    expect(form).not.toContain("Choose an agent");
    expect(startOf(choice(hosts, onIssue), onIssue)).toStrictEqual({
      Session: {
        Start: {
          at: { Issue: issue.id },
          effort: null,
          instance: { account: "Own", provider: "claude" },
          model: null,
        },
      },
    });
  });

  it("requires a choice between eligible hosts and keeps the issue on either", () => {
    expect.hasAssertions();
    const hosts = [prepared("laptop"), prepared("devbox")];
    const options = eligible(hosts, onIssue);
    expect(choice(hosts, onIssue)).toBeUndefined();
    expect(options.map((one) => one.label)).toStrictEqual([
      "Claude Code on laptop",
      "Claude Code on devbox",
    ]);
    for (const one of options) {
      const draft = { ...onIssue, agent: one.key };
      const chosen = choice(hosts, draft);
      expect(chosen?.host.key).toBe(one.host.key);
      expect(startOf(chosen, draft)).toMatchObject({
        Session: { Start: { at: { Issue: issue.id } } },
      });
    }
  });
});

describe("session accounts", () => {
  it("distinguishes two hosts that advertise the same name", () => {
    expect.hasAssertions();
    const devbox = prepared("devbox");
    const other = { ...devbox, key: "laptop" };
    const labels = eligible([devbox, other], onIssue).map((one) => one.label);
    expect(new Set(labels).size).toBe(labels.length);
  });

  it("offers named accounts separately and excludes existing assignments", () => {
    expect.hasAssertions();
    const host = prepared("devbox");
    const fold = turnThrough(4);
    const instances = namedInstances(host);
    const assigned = { ...sessionOf(fold, session), issue: issue.id };
    const sessions = new Map([[session, assigned]]);
    const available = eligible([{ ...host, fold: { ...host.fold, instances, sessions } }], onIssue);
    expect(available.map((one) => one.label)).toStrictEqual(["Claude Code (work) on devbox"]);
  });
});

describe("session locations", () => {
  it("starts in a directory on the selected host", () => {
    expect.hasAssertions();
    const hosts = [prepared("laptop"), prepared("devbox")];
    const draft = { ...blank, directory, effort: "Low", model: "small" };
    const chosen = selection(eligible(hosts, draft, "laptop"), "");
    expect(chosen?.host.key).toBe("laptop");
    const { models } = Option.getOrThrow(Option.fromUndefinedOr(chosen));
    expect(effortsOf(models, "")).toStrictEqual(["Low", "High"]);
    expect(startOf(chosen, draft)).toStrictEqual({
      Session: {
        Start: {
          at: { Directory: directory },
          effort: "Low",
          instance: { account: "Own", provider: "claude" },
          model: "small",
        },
      },
    });
    expect(startOf(chosen, blank)).toBeUndefined();
  });

  it("starts in the selected project without an issue", () => {
    expect.hasAssertions();
    const host = prepared("devbox");
    const inProject = { ...blank, project: projectId };
    const projectAgent = choice([host], inProject);
    expect(startOf(projectAgent, inProject)).toMatchObject({
      Session: { Start: { at: { Project: projectId } } },
    });
  });
});
