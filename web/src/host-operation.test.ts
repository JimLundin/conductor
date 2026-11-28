import { HostAction, HostOperation } from "@conductor/core/pairing";
import { describe, expect, it } from "@effect/vitest";
import { initialView, requestHost, settledHost } from "@conductor/core/store";
import { Result } from "effect";
import { emptyHost } from "@conductor/core/fold/host";
import example from "#ticket-example" with { type: "json" };
import { hostKey } from "@conductor/core/saved";
import { parseJoin } from "@conductor/core/ticket";

const paired = (): ReturnType<typeof requestHost> => {
  const ticket = Result.getOrThrow(parseJoin(example.text));
  return requestHost(
    initialView(),
    {
      fold: emptyHost(),
      key: hostKey(ticket),
      saved: undefined,
      status: { error: undefined, state: "Disconnected" },
      ticket,
    },
    HostAction.Pair({ fragment: "" }),
  );
};

describe("retried operations", () => {
  it("a retry racing an earlier attempt of the same action settles only the retry", () => {
    expect.hasAssertions();
    const first = paired();
    const attempt = first.host.operation;
    const failedView = settledHost(
      first.view,
      first.host.key,
      HostOperation.Failed({ action: attempt.action, id: attempt.id }),
    );
    const retried = requestHost(failedView, first.host, attempt.action);
    const retry = retried.host.operation;
    const late = settledHost(
      retried.view,
      first.host.key,
      HostOperation.Complete({ action: attempt.action, id: attempt.id }),
    );
    expect(late.hosts.get(first.host.key)?.operation).toStrictEqual(retry);
    const settled = settledHost(
      late,
      first.host.key,
      HostOperation.Complete({ action: retry.action, id: retry.id }),
    );
    expect(settled.hosts.get(first.host.key)?.operation._tag).toBe("Complete");
  });
});
