import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// Relative asset paths, so the build serves from Pages' /conductor/ and from any other root.
// `--mode full` runs the property tests at the full profile's case count.
export default defineConfig(({ mode }) => ({
  base: "./",
  plugins: [react()],
  test: {
    setupFiles: [new Map([["full", "./profiles/full.ts"]]).get(mode) ?? "./profiles/fast.ts"],
  },
}));
