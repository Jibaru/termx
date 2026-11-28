import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { App, createHome, removeHome } from "../src/app.ts";
import { chord } from "../src/keys.ts";
import { inFront, newShell, stepTimer } from "../src/sessions.ts";
import { Sidebar } from "../src/sidebar.ts";
import { waitFor } from "../src/wait.ts";

/** The shortcut that goes to the session waiting longest. */
const JUMP_TO_WAITING =
  process.platform === "darwin"
    ? chord({ key: "a", code: "KeyA", keyCode: 65 }, { meta: true, shift: true })
    : chord({ key: "A", code: "KeyA", keyCode: 65 }, { ctrl: true, shift: true });

/**
 * Which session needs the user, end to end: consent given and the plugin
 * installed from the window, into the Claude Code configuration of the
 * empty home directory of the run; a stand-in for Claude Code (the
 * `fake-claude` helper) runs the hooks of the plugin in a terminal of
 * termx; the sidebar, the "Needs you" list and the jump shortcut follow.
 * The real Claude Code is never run.
 *
 * Only in CI: it installs a plugin into a configuration of Claude Code,
 * which on a machine of a developer is better left to a deliberate run.
 */
describe.runIf(process.env.CI !== undefined)("a session that needs the user", () => {
  let home: string | undefined;
  let app: App | undefined;

  beforeAll(async () => {
    home = await createHome();
    // What makes termx believe Claude Code is installed: its user settings.
    // termx asks for consent only then.
    mkdirSync(join(home, ".claude"), { recursive: true });
    writeFileSync(join(home, ".claude", "settings.json"), "{}\n");
    app = await App.launch("attention", { home });
  }, 300_000);

  afterAll(async () => {
    await app?.dispose();
    if (home !== undefined) await removeHome(home);
  }, 120_000);

  function running(): { app: App; home: string } {
    if (!app || home === undefined) throw new Error("the application is not running");
    return { app, home };
  }

  /** What one row of the sidebar shows about its agent. */
  function rowOf(id: string) {
    return running().app.driver.execute<{
      agent: string | undefined;
      attention: boolean;
      queued: boolean;
    }>(
      `const id = arguments[0];
       const row = document.querySelector('[data-testid="session-row"][data-session-id="' + id + '"]');
       return {
         agent: row?.querySelector('[data-testid="agent-slot"]')?.dataset.status,
         attention: row?.dataset.attention === "true",
         queued: document.querySelector('[data-testid="attention-item"][data-session-id="' + id + '"]') !== null,
       };`,
      id,
    );
  }

  async function waitForRow(
    id: string,
    what: string,
    check: (row: Awaited<ReturnType<typeof rowOf>>) => boolean,
  ): Promise<void> {
    let last: Awaited<ReturnType<typeof rowOf>> | undefined;
    await waitFor(
      async () => {
        last = await rowOf(id);
        return check(last);
      },
      { what, describe: () => last },
    );
  }

  let hooksJson = "";

  it("asks, and installs the plugin once the user agrees", async () => {
    const { app, home } = running();
    const sidebar = new Sidebar(app.driver);

    await sidebar.clickWhenThere('[data-testid="claude-consent-open"]');
    // Install is pressed only once the list of changes is on screen.
    await waitFor(
      () =>
        app.driver.execute<boolean>(
          `const install = document.querySelector('[data-testid="claude-install"]');
       return document.querySelector('[data-testid="claude-changes"]') !== null &&
         install !== null && !install.disabled;`,
        ),
      { what: "the list of changes, and Install" },
    );
    await sidebar.click('[data-testid="claude-install"]');

    await waitFor(() => sidebar.has('[data-testid="claude-status"][data-state="installed"]'), {
      what: "the sidebar to say the integration is installed",
    });
    const settings = readFileSync(join(home, ".claude", "settings.json"), "utf8");
    expect(JSON.parse(settings)).toMatchObject({ enabledPlugins: { "termx@termx": true } });
    hooksJson = join(home, "termx-state", "claude-plugin", "termx", "hooks", "hooks.json");
  });

  it("is listed, marked, reached with the shortcut, and leaves the list once it works", async () => {
    const { app } = running();
    const { terminal } = app;
    expect(hooksJson, "the plugin was not installed: see the test above").not.toBe("");
    const step = stepTimer("attention");
    const sidebar = new Sidebar(app.driver);
    const claude = (await terminal.state()).session;

    const { id } = await terminal.startHelper("fake-claude", `"${hooksJson}"`);
    await terminal.waitForExactLine(`FAKE-CLAUDE-${id}:SessionStart`);
    const fire = async (event: string) => {
      await terminal.submit(event);
      await terminal.waitForExactLine(`FAKE-CLAUDE-${id}:${event}`);
    };
    await fire("UserPromptSubmit");
    await waitForRow(claude, "the agent to work", (row) => row.agent === "working");
    expect(await rowOf(claude)).toMatchObject({ attention: false, queued: false });

    await fire("PermissionRequest");
    await waitForRow(
      claude,
      "the session to ask for permission, marked and listed",
      (row) => row.agent === "waitingForPermission" && row.attention && row.queued,
    );
    step("waiting for permission");

    // Another session in front: the waiting one is out of sight.
    const other = await newShell(sidebar, terminal);
    expect(other).not.toBe(claude);

    await terminal.keys(JUMP_TO_WAITING);
    await inFront(
      terminal,
      "the session that waits to be in front",
      (state) => state.session === claude && state.status.kind === "running",
    );
    step("jumped");

    // Back at work: no longer listed, no longer marked.
    await fire("PostToolUse");
    await waitForRow(
      claude,
      "the session to work again, off the list",
      (row) => row.agent === "working" && !row.attention && !row.queued,
    );
    expect(
      await app.driver.execute<boolean>(
        `return document.querySelector('[data-testid="attention-queue"]') === null;`,
      ),
    ).toBe(true);
  });
});
