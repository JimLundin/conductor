import { Array, Option } from "effect";
import { blank, choosable, effortsOf, optional, startOf } from "@conductor/core/session/start";
import { describe, expect, it } from "@effect/vitest";
import { directory, turnThrough } from "#ui/test/session-events.ts";
import { newSessionForm } from "#ui/test/new-session.tsx";
import { view } from "#ui/test/session.tsx";

describe("starting a session", () => {
  it("lists available instances on the chosen host", () => {
    expect.hasAssertions();
    const before = turnThrough(0);
    const form = newSessionForm(before);
    expect(form).toContain('<option value="laptop" selected="">test-host</option>');
    expect(form).toContain('<option value="claude">Claude Code</option>');
    expect(form).not.toContain("Codex");
  });

  it("starts in the selected directory", () => {
    expect.hasAssertions();
    const before = turnThrough(0);
    const draft = { ...blank, directory, host: "laptop", instance: "claude" };
    expect(startOf(before, draft)).toStrictEqual({
      Session: {
        Start: {
          at: { Directory: directory },
          effort: null,
          instance: { account: "Own", provider: "claude" },
          model: null,
        },
      },
    });
  });

  it("opens ready for a message", () => {
    expect.hasAssertions();
    const host = turnThrough(4);
    const html = view(host);
    expect(html).toContain("Send a message to start.");
    expect(html).toContain('<button class="primary" type="submit">Send</button>');
  });
});

describe("new session choices", () => {
  it("offers the instance's models and their efforts", () => {
    expect.hasAssertions();
    const before = turnThrough(0);
    const choices = choosable(before);
    const { models } = Option.getOrThrow(Array.head(choices));
    expect(models.choices.map((choice) => choice.id)).toStrictEqual(["big", "small"]);
    const defaultModel = optional(blank.model);
    expect(effortsOf(models, defaultModel)).toStrictEqual(["Low", "High"]);
  });

  it("starts in a project with the selected model and effort", () => {
    expect.hasAssertions();
    const before = turnThrough(0);
    const draft = {
      ...blank,
      effort: "Low",
      instance: "claude",
      model: "small",
      project: "0twr04e5",
    };
    expect(startOf(before, draft)).toStrictEqual({
      Session: {
        Start: {
          at: { Project: "0twr04e5" },
          effort: "Low",
          instance: { account: "Own", provider: "claude" },
          model: "small",
        },
      },
    });
  });

  it("refuses a missing place or an unavailable instance", () => {
    expect.hasAssertions();
    const before = turnThrough(0);
    const draft = { ...blank, effort: "Low", instance: "claude", model: "small" };
    expect(startOf(before, draft)).toBeUndefined();
    expect(startOf(before, { ...draft, directory, instance: "codex" })).toBeUndefined();
  });
});
