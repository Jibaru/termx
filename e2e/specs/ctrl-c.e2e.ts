import { describe, expect, it } from "vitest";

import { useApp } from "../src/fixture.ts";
import { isAlive } from "../src/processes.ts";
import type { Terminal } from "../src/terminal.ts";
import { delay } from "../src/wait.ts";

/** How long a program may keep running after Ctrl+C, in milliseconds. */
const MAX_INTERRUPT_MS = 10_000;

/**
 * Starts a program that does nothing and presses Ctrl+C, checking each step:
 * the program runs, and Ctrl+C reached the session of the terminal as the
 * byte 03. Returns the process identifier of the program.
 */
async function pressCtrlCOnIdleProgram(terminal: Terminal): Promise<number> {
  const { id } = await terminal.startHelper("hold");
  const programPid = await terminal.waitForLine("the program to run", (line) => {
    const match = new RegExp(`^HOLD-${id}:shell=\\d+:self=(\\d+)$`).exec(line);
    return match ? Number(match[1]) : undefined;
  });
  expect(isAlive(programPid), "the program runs").toBe(true);

  await terminal.recordInput();
  await terminal.interrupt();

  expect(await terminal.recordedInput(), "the input Ctrl+C produced").toBe("03");
  expect((await terminal.state()).status.kind, "the session the input went to").toBe("running");
  return programPid;
}

/**
 * Whether the process is still there `MAX_INTERRUPT_MS` after Ctrl+C. Waits
 * less if it ends sooner. Never fails.
 */
async function survivesCtrlC(pid: number): Promise<boolean> {
  const deadline = Date.now() + MAX_INTERRUPT_MS;
  while (isAlive(pid) && Date.now() < deadline) await delay(50);
  return isAlive(pid);
}

describe("Ctrl+C", () => {
  const fixture = useApp("ctrl-c");

  it("interrupts the program in the terminal", async () => {
    const { terminal } = fixture;
    const programPid = await pressCtrlCOnIdleProgram(terminal);
    expect(await survivesCtrlC(programPid), "the program is still running").toBe(false);
    await terminal.waitForPrompt();
  });
});

// A process can be started with Ctrl+C ignored (the flag
// CREATE_NEW_PROCESS_GROUP of CreateProcess does it), and every process it
// starts inherits that. CI runners, some launchers and `start /b` start
// programs that way.
describe.runIf(process.platform === "win32")(
  "Ctrl+C, when termx was started with it ignored",
  () => {
    const fixture = useApp("ctrl-c-ignored", { ignoringCtrlC: true });
    let programPid: number | undefined;

    // Everything up to the interruption is checked on its own, so that a
    // failure says which step broke.
    it("delivers Ctrl+C to the session of the terminal", async () => {
      programPid = await pressCtrlCOnIdleProgram(fixture.terminal);
    });

    // termx gives its terminals the default handling of Ctrl+C back before
    // it starts them (`SetConsoleCtrlHandler(NULL, FALSE)`, in
    // crates/termx-core/src/pty/ctrl_c.rs). Without that, the program would
    // still run 10 s after Ctrl+C.
    it("interrupts the program in the terminal", async () => {
      if (programPid === undefined) throw new Error("Ctrl+C was not delivered: see the test above");
      const survived = await survivesCtrlC(programPid);
      expect(survived, "the program is still running after Ctrl+C").toBe(false);
    });
  },
);
