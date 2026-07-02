import { defineConfig } from "vitest/config";

// Pure-function unit tests only (formatters + derived helpers). No DOM needed,
// so the default `node` environment is fine and keeps the suite fast.
export default defineConfig({
  test: {
    include: ["src/**/*.test.ts"],
    environment: "node",
  },
});
