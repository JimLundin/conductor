import { button, view } from "#ui/test/session.tsx";
import {
  created,
  foldEvents,
  sessionOf,
  started,
  subagentStarted,
  subagentTool,
  user,
} from "#ui/test/examples.ts";
import { describe, expect, it } from "@effect/vitest";
import { directory, session, turnThrough } from "#ui/test/session-events.ts";
import { message } from "@conductor/core/session/commands";

describe("a session message", () => {
  it("sends the text to the selected session", () => {
    expect.hasAssertions();
    const ready = turnThrough(4);
    expect(view(ready)).toContain('<textarea aria-label="Message"');
    expect(message(session, "create a.txt")).toStrictEqual({
      Session: { SendMessage: { session, text: "create a.txt" } },
    });
  });

  it("names the account in the session header", () => {
    expect.hasAssertions();
    const host = turnThrough(4);
    const record = {
      ...sessionOf(host, session),
      instance: { account: { Named: "work" }, provider: "claude" },
    };
    const sessions = new Map([[session, record]]);
    expect(view({ ...host, sessions })).toContain("claude (work)");
  });

  it("shows the message and tool card while waiting for approval", () => {
    expect.hasAssertions();
    const waiting = turnThrough(8);
    const html = view(waiting);
    expect(html).toContain('<p class="bubble">create a.txt</p>');
    expect(html).toContain("<code>/tmp/s1-scratch/a.txt</code>");
    expect(html).toContain('<p class="approval open">Waiting for approval</p>');
    expect(html).toContain('disabled="" type="submit">Send</button>');
  });

  it("offers only the approval choices supplied by the host", () => {
    expect.hasAssertions();
    const waiting = turnThrough(8);
    const html = view(waiting);
    expect(html).toContain(button("Approve"));
    expect(html).toContain(button("Deny"));
    expect(html).not.toContain("Approve for session");
  });
});

describe("a finished session turn", () => {
  it("shows its completed tool, reply and outcome", () => {
    expect.hasAssertions();
    const finished = turnThrough(13);
    const html = view(finished);
    expect(html).toContain('data-status="Succeeded"');
    expect(html).toContain(
      "<p><span>I created </span><code>a.txt</code><span> in </span><code>/tmp/s1-scratch</code>",
    );
    expect(html).toContain("Turn completed");
  });
});

describe("session activity", () => {
  it("nests a subagent's tool cards under the tool call that started it", () => {
    expect.hasAssertions();
    const host = foldEvents([
      [1, created(directory)],
      [1, "SessionReady"],
      [1, user("look around")],
      [1, started("toolu_9")],
      [1, subagentStarted("scout", "toolu_9")],
      [
        1,
        subagentTool("scout", "toolu_10", {
          Started: { tool: { Command: { command: "rg todo" } } },
        }),
      ],
    ]);
    const html = view(host);
    expect(html).toMatch(
      /<article class="tool running"[^>]*>(?:(?!<\/article>).)*look for scout(?:(?!<\/article>).)*rg todo/su,
    );
  });

  it("shows a streaming reply while its deltas arrive", () => {
    expect.hasAssertions();
    const host = foldEvents([
      [1, created(directory)],
      [1, "SessionReady"],
      [1, user("hi")],
      [1, { Agent: { AssistantDelta: { text: "Hel" } } }],
      [1, { Agent: { AssistantDelta: { text: "lo" } } }],
    ]);
    expect(view(host)).toContain(
      '<div class="reply streaming"><div class="markdown"><p><span>Hello</span>',
    );
  });
});
