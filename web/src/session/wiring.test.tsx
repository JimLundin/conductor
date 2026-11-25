import { describe, expect, it } from "@effect/vitest";
import { approvalThroughStore } from "#ui/test/session-wiring.tsx";
import { Effect } from "effect";

describe("the session route", () => {
  it.effect("shows the store's session and the host's approval answer", () =>
    Effect.gen(function* approval() {
      expect.hasAssertions();
      const { before, after } = yield* approvalThroughStore();
      expect(before).toContain('<div class="shell" data-top="pane">');
      expect(before).toContain('<p class="meta"><span>test-host</span>');
      expect(before).toContain('type="button">Approve</button>');
      expect(after).toContain("Approved by test-client");
    }),
  );
});
