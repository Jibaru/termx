import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["specs/**/*.e2e.ts"],
    // One application at a time: they would fight over the keyboard focus
    // and the clipboard, and CI machines have few cores.
    fileParallelism: false,
    // A test that fails is never repeated: a flaky test is a failure. Only
    // the start of the application is, in `src/app.ts`.
    retry: 0,
    testTimeout: 240_000,
    hookTimeout: 300_000,
    // Tests run slowly by nature here.
    slowTestThreshold: 30_000,
  },
});
