import { beforeAll, inject } from "vitest";
import { configureGlobal } from "effect/Arbitrary";

declare module "vitest" {
  interface ProvidedContext {
    readonly arbitrary: { readonly runs: number; readonly seed?: number };
  }
}

// Property tests take the runs and seed of the mode from vitest.config.ts.
beforeAll(() => {
  configureGlobal({ check: inject("arbitrary") });
});
