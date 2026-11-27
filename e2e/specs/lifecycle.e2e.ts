import { describe, expect, it } from "vitest";

import { useApp } from "../src/fixture.ts";
import { enter } from "../src/keys.ts";
import { isAlive } from "../src/processes.ts";
import { waitFor } from "../src/wait.ts";

describe("the end of a shell", () => {
  const fixture = useApp("exit");

  it("shows the exit code and starts a new shell on Enter", async () => {
    const { terminal } = fixture;
    const before = await terminal.state();

    await terminal.submit("exit 3");

    const exited = await terminal.waitForState(
      "the terminal to notice that the shell exited",
      (state) => state.status.kind === "exited",
    );
    expect(exited.status.exit).toMatchObject({ code: 3, signal: null });
    await terminal.waitForLine("the exit notice", (line) =>
      line.includes("The process exited with code 3. Press Enter to restart."),
    );

    // Any other key does nothing: no new session, nothing written.
    const beforeKey = await terminal.state();
    const linesBefore = await terminal.lines();
    await terminal.type("x");
    // The key is handled as it is delivered; a turn of the event loop of
    // the page lets anything it would have started happen.
    await terminal.ping();
    const afterKey = await terminal.state();
    // An exited terminal runs nothing: the same status means that nothing
    // was started.
    expect(afterKey.status).toEqual(exited.status);
    expect(afterKey.bufferLength).toBe(beforeKey.bufferLength);
    expect(await terminal.lines()).toEqual(linesBefore);

    // Enter starts a new shell, which the `x` never reaches.
    await terminal.keys(enter());
    const restarted = await terminal.waitForState(
      "a new shell to be running",
      (state) => state.status.kind === "running",
    );
    // The same session of the workspace, in a new terminal.
    expect(restarted.session).toBe(before.session);
    expect(restarted.status.terminal).not.toBe(before.status.terminal);
    await terminal.waitForPrompt();
    const typedAtPrompt = (await terminal.lines()).filter((line) => /^termx-e2e[$>] x/.test(line));
    expect(typedAtPrompt).toEqual([]);
  });
});

describe("quitting the application", () => {
  const fixture = useApp("quit");

  it("leaves no shell and no program of a terminal behind", async () => {
    const { app, terminal } = fixture;
    // A program in the foreground of the shell, as Claude Code would be.
    const { id } = await terminal.startHelper("hold");
    const [shellPid, programPid] = await terminal.waitForLine("the process identifiers", (line) => {
      const match = new RegExp(`^HOLD-${id}:shell=(\\d+):self=(\\d+)$`).exec(line);
      return match ? [Number(match[1]), Number(match[2])] : undefined;
    });
    expect(isAlive(shellPid)).toBe(true);
    expect(isAlive(programPid)).toBe(true);
    expect(isAlive(app.pid)).toBe(true);

    const exit = await app.quit();

    expect(exit).toEqual({ code: 0, signal: null });
    // Checked from the outside, once the application is gone.
    await waitFor(() => !isAlive(shellPid) && !isAlive(programPid), {
      what: "the shell and its program to be gone",
      describe: () => ({ shell: isAlive(shellPid), program: isAlive(programPid) }),
    });
  });
});
