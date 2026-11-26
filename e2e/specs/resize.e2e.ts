import { describe, it } from "vitest";

import { useApp } from "../src/fixture.ts";
import type { TerminalState } from "../src/page.ts";
import type { Terminal } from "../src/terminal.ts";
import { waitFor } from "../src/wait.ts";

/**
 * Waits until the program started with `startHelper("size")` has reported
 * the size xterm.js has now, and that size is not `previous`.
 */
function waitForReportedSize(
  terminal: Terminal,
  id: string,
  previous: Pick<TerminalState, "cols" | "rows">,
): Promise<TerminalState> {
  let state: TerminalState | undefined;
  let reported: string[] = [];
  return waitFor(
    async () => {
      state = await terminal.state();
      if (state.cols === previous.cols && state.rows === previous.rows) return undefined;
      reported = (await terminal.lines()).filter((line) => line.startsWith(`SIZE-${id}:`));
      const expected = `SIZE-${id}:${String(state.cols)}x${String(state.rows)}`;
      // The last report is the size the program has now.
      return reported.at(-1) === expected ? state : undefined;
    },
    {
      what: "the program in the terminal to see the new size",
      describe: () => ({ previous, terminal: state, reported }),
    },
  );
}

describe("resizing the window", () => {
  const fixture = useApp("resize");

  it("changes the size the program in the terminal sees", async () => {
    const { app, terminal } = fixture;
    const initial = await terminal.state();
    const { id } = await terminal.startHelper("size");
    await terminal.waitForExactLine(`SIZE-${id}:${String(initial.cols)}x${String(initial.rows)}`);

    // Smaller, then larger again: the program is told both times, while it
    // runs. Both sizes fit on the smallest display of a CI machine.
    await app.driver.setWindowSize(900, 600);
    const smaller = await waitForReportedSize(terminal, id, initial);
    await app.driver.setWindowSize(1000, 660);
    await waitForReportedSize(terminal, id, smaller);

    await terminal.type("q");
    await terminal.waitForExactLine(`SIZE-EXIT-${id}`);
  });

  it.skipIf(process.platform === "win32")("is what stty reports", async () => {
    const { terminal } = fixture;
    await terminal.waitForPrompt();

    await terminal.submit("stty size");

    let state: TerminalState | undefined;
    await terminal.waitForLine("the size printed by stty", async (line) => {
      state = await terminal.state();
      return line === `${String(state.rows)} ${String(state.cols)}`;
    });
  });
});
