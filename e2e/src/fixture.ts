import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { afterAll, beforeAll, beforeEach } from "vitest";

import { App, type LaunchOptions } from "./app.ts";
import type { Terminal } from "./terminal.ts";

/** How long starting or stopping the application may take, in milliseconds. */
const LIFECYCLE_TIMEOUT_MS = 300_000;

/** What a test file gets from `useApp`. Only valid inside its tests. */
export interface AppFixture {
  readonly app: App;
  readonly terminal: Terminal;
}

function fileName(text: string): string {
  return text.replaceAll(/[^a-z0-9]+/gi, "-").slice(0, 80);
}

/** Keeps what helps to understand a failure: what the terminal showed. */
async function captureFailure(app: App, test: string): Promise<void> {
  const dir = join(app.artifactsDir, "failures");
  mkdirSync(dir, { recursive: true });
  const base = join(dir, fileName(test));
  const attempts: [string, () => Promise<string | Buffer>][] = [
    ["terminal.txt", async () => (await app.terminal.lines(2_000)).join("\n")],
    ["state.json", async () => JSON.stringify(await app.terminal.state(), null, 2)],
    ["screenshot.png", () => app.driver.screenshot()],
  ];
  for (const [suffix, read] of attempts) {
    try {
      writeFileSync(`${base}.${suffix}`, await read());
    } catch (error) {
      writeFileSync(`${base}.${suffix}.error.txt`, String(error));
    }
  }
}

/**
 * Starts the application once for the test file that calls this, and stops
 * it afterwards. `name` names the run and its directory under `artifacts/`.
 *
 * The tests of a file share one application and one terminal, in order.
 * They tell their output apart with tokens (`terminal.token()`), not by
 * starting from an empty screen.
 */
export function useApp(name: string, options: LaunchOptions = {}): AppFixture {
  let app: App | undefined;

  beforeAll(async () => {
    app = await App.launch(name, options);
  }, LIFECYCLE_TIMEOUT_MS);

  beforeEach(({ task, onTestFailed }) => {
    onTestFailed(async () => {
      if (app) await captureFailure(app, task.name);
    });
  });

  afterAll(async () => {
    await app?.dispose();
  }, LIFECYCLE_TIMEOUT_MS);

  const running = (): App => {
    if (!app) throw new Error("the application is not running");
    return app;
  };
  return {
    get app() {
      return running();
    },
    get terminal() {
      return running().terminal;
    },
  };
}
