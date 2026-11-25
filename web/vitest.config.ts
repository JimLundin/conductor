import type { ConfigEnv } from "vitest/config";
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Each mode's property-test runs and seed, and the failures a run ends on.
// A mode is vitest's --mode; its default, test, is Stryker's, so mutation.
const modes = {
  fast: { bail: 0, runs: 10 },
  full: { bail: 0, runs: 1000 },
  mutation: { bail: 1, runs: 10, seed: 42 },
} as const;

type Mode = keyof typeof modes;

const isMode = (mode: string): mode is Mode => Object.hasOwn(modes, mode);

export default defineConfig(({ mode }: Readonly<ConfigEnv>) => {
  const chosen: Mode = isMode(mode) ? mode : "mutation";
  const { bail, ...arbitrary } = modes[chosen];
  const full = chosen === "full";
  return {
    plugins: [react()],
    resolve: { tsconfigPaths: true },
    test: {
      bail,
      coverage: { enabled: full, provider: "v8" as const, reporter: ["text"] },
      provide: { arbitrary },
      // The setup file beside this config, here or in check.sh's copy of it.
      setupFiles: [new URL("vitest.setup.ts", import.meta.url).href],
      tags: [{ description: "runs only in the full mode", name: "slow" }],
      tagsFilter: full ? [] : ["!slow"],
    },
  };
});
