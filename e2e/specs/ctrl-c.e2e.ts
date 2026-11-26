import { describe, it } from "vitest";

import { useApp } from "../src/fixture.ts";
import type { Terminal } from "../src/terminal.ts";

/** How long a program may keep running after Ctrl+C, in milliseconds. */
const MAX_INTERRUPT_MS = 10_000;

/** Starts a program that does nothing, presses Ctrl+C, and waits for the shell. */
async function interruptIdleProgram(terminal: Terminal): Promise<void> {
  const { id } = await terminal.startHelper("hold");
  await terminal.waitForLine("the program to run", (line) => line.startsWith(`HOLD-${id}:`));

  await terminal.interrupt();

  await terminal.waitForPrompt(MAX_INTERRUPT_MS);
}

describe("Ctrl+C", () => {
  const fixture = useApp("ctrl-c");

  it("interrupts the program in the terminal", async () => {
    await interruptIdleProgram(fixture.terminal);
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

    // EXPECTED FAILURE, a bug of the application: termx passes the ignored
    // Ctrl+C on to the shells of its terminals, and Ctrl+C then interrupts
    // nothing in them. A terminal should give its shells the default handling
    // back (SetConsoleCtrlHandler(NULL, FALSE) before it starts them).
    // Explained in https://github.com/Jibaru/termx/pull/6, under "Bugs of the
    // application found by the suite". Remove `.fails` with the fix: this
    // test then starts failing, which is the reminder.
    it.fails("interrupts the program in the terminal", async () => {
      await interruptIdleProgram(fixture.terminal);
    });
  },
);
