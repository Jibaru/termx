import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { useApp } from "../src/fixture.ts";
import { Sidebar } from "../src/sidebar.ts";
import { waitFor } from "../src/wait.ts";

/**
 * The Claude Code integration, end to end: the plugin is installed into
 * the empty home directory of the run, a stand-in for Claude Code runs the
 * hook command it installed, with the input Claude Code gives a hook, in a
 * terminal of termx, and the row of the session in the sidebar follows.
 * The real Claude Code is never run.
 */
describe("the agent state of a session", () => {
  const fixture = useApp("agent");

  /** What the agent slot of the only row of the sidebar shows. */
  async function agentStatus(): Promise<string | undefined> {
    return fixture.app.driver.execute<string | undefined>(
      `return document.querySelector('[data-testid="session-row"] [data-testid="agent-slot"]')?.dataset.status;`,
    );
  }

  async function waitForAgent(status: string): Promise<void> {
    let last: string | undefined;
    await waitFor(
      async () => {
        last = await agentStatus();
        return last === status;
      },
      { what: `the agent of the session to be ${status}`, describe: () => last },
    );
  }

  /** Invokes a command of the application from the page, and waits for it. */
  async function invoke(command: string): Promise<unknown> {
    const { driver } = fixture.app;
    const key = `__termxE2e_${command}`;
    await driver.execute(
      `const [command, key] = arguments;
       window[key] = undefined;
       window.__TAURI_INTERNALS__.invoke(command).then(
         (value) => { window[key] = { ok: true, value: value ?? null }; },
         (error) => { window[key] = { ok: false, error }; },
       );`,
      command,
      key,
    );
    const result = await waitFor(
      () =>
        driver.execute<{ ok: boolean; value?: unknown; error?: unknown } | null>(
          `return window[arguments[0]] ?? null;`,
          key,
        ),
      { what: `${command} to answer` },
    );
    if (!result.ok) throw new Error(`${command} failed: ${JSON.stringify(result.error)}`);
    return result.value;
  }

  let hooksJson = "";

  it("installs the plugin with the hook sender of termx", async () => {
    const before = (await invoke("claude_integration_status")) as { plugin: string };
    expect(before.plugin).toBe("notInstalled");

    await invoke("claude_integration_install");

    const after = (await invoke("claude_integration_status")) as { plugin: string };
    expect(after.plugin).toBe("installed");
    // In the state directory of the run (TERMX_STATE_DIR).
    hooksJson = join(
      fixture.app.home,
      "termx-state",
      "claude-plugin",
      "termx",
      "hooks",
      "hooks.json",
    );
    expect(existsSync(hooksJson), hooksJson).toBe(true);
    const settings = readFileSync(join(fixture.app.home, ".claude", "settings.json"), "utf8");
    expect(JSON.parse(settings)).toMatchObject({ enabledPlugins: { "termx@termx": true } });
  });

  it("follows a turn: working, waiting for permission, working, waiting for input", async () => {
    const { terminal } = fixture;
    expect(hooksJson, "the plugin was not installed: see the test above").not.toBe("");
    const sidebar = new Sidebar(fixture.app.driver);
    await sidebar.waitForRows("the session", (rows) => rows.length === 1);
    expect(await agentStatus()).toBe("none");

    // The terminal started before the plugin was installed: its programs
    // still have what they need to report.
    const { id } = await terminal.startHelper("fake-claude", `"${hooksJson}"`);
    await terminal.waitForExactLine(`FAKE-CLAUDE-${id}:SessionStart`);
    await waitForAgent("unknown");

    const step = async (event: string, status: string) => {
      await terminal.submit(event);
      await terminal.waitForExactLine(`FAKE-CLAUDE-${id}:${event}`);
      await waitForAgent(status);
    };
    await step("UserPromptSubmit", "working");
    await step("PreToolUse", "working");
    await step("PermissionRequest", "waitingForPermission");
    await step("PostToolUse", "working");
    await step("AskUserQuestion", "waitingForInput");

    // The hooks said nothing, on any of their outputs.
    const lines = await terminal.lines(400);
    expect(lines.filter((text) => text.includes(`FAKE-CLAUDE-NOISY-${id}`))).toEqual([]);

    // Claude Code killed, without a final hook: termx no longer vouches
    // for the state.
    await terminal.submit("die");
    await waitForAgent("unknown");
    await terminal.waitForPrompt();
  });
});
