import path from "node:path";
import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  test: {
    // Logic tests (`*.test.ts`) run in the fast Node env; render-level
    // component tests (`*.test.tsx`) opt into jsdom per-file with a
    // `// @vitest-environment jsdom` docblock, so the Node suite stays quick.
    include: ["src/**/*.test.{ts,tsx}"],
    environment: "node",
    passWithNoTests: true,
  },
});
