import { Effect, Exit, Scope } from "effect";
import { describe, expect, it } from "@effect/vitest";
import { AtomRegistry } from "effect/reactivity";
import example from "#ticket-example" with { type: "json" };
import { lifecyclePage } from "#ui/test/lifecycle.ts";
import { makeStore } from "#ui/store.ts";

const recordKey = `conductor.v2.host.58${"66".repeat(31)}`;

describe("store scope", () => {
  it.effect("store scope closure interrupts pending work even with an external atom registry", () =>
    Effect.scoped(
      Effect.gen(function* closing() {
        const page = yield* lifecyclePage();
        const scope = yield* Scope.make();
        const registry = AtomRegistry.make();
        yield* Effect.addFinalizer(() =>
          Effect.sync(() => {
            registry.dispose();
          }),
        );
        const store = yield* Scope.provide(scope)(makeStore(page.boundary, registry));
        const write = yield* page.holdWrite;
        store.pair(example.text);
        yield* write.started;
        yield* Effect.gen(function* disposed() {
          yield* Scope.close(scope, Exit.void);
          yield* write.release;
          yield* Effect.yieldNow;
          expect(yield* page.connected).toBe(0);
          expect(page.memory.getItem(recordKey) ?? undefined).toBeUndefined();
        });
      }),
    ),
  );
});
