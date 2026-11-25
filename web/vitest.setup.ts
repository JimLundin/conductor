import { beforeAll, inject } from "vitest";
import { configureGlobal } from "effect/Arbitrary";

declare module "vitest" {
  interface ProvidedContext {
    readonly arbitrary: { readonly runs: number; readonly seed?: number };
  }
}

beforeAll(() => {
  configureGlobal({ check: inject("arbitrary") });
});
