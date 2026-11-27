import type { TerminalState } from "./page.ts";
import type { Sidebar } from "./sidebar.ts";
import type { Terminal } from "./terminal.ts";
import { waitFor } from "./wait.ts";

/**
 * What the specs about several sessions share: waiting for the terminal in
 * front, starting another shell from the dialog, reading the ticks of the
 * `ticks` helper, and timing the steps of a spec.
 */

/**
 * Waits for the terminal in front to satisfy `check`. While the focused
 * pane is empty, or the page loads again, there is no terminal in front,
 * which is not a failure.
 */
export function inFront(
  terminal: Terminal,
  what: string,
  check: (state: TerminalState) => boolean,
  timeoutMs?: number,
): Promise<TerminalState> {
  let last: unknown;
  return waitFor(
    async () => {
      try {
        const state = await terminal.state();
        last = state;
        return check(state) ? state : undefined;
      } catch (error) {
        last = String(error);
        return undefined;
      }
    },
    { what, describe: () => last, timeoutMs },
  );
}

/**
 * Starts another shell from the new-session dialog, in the most recent
 * folder, and waits for it to be in front at its prompt. Returns its id.
 */
export async function newShell(sidebar: Sidebar, terminal: Terminal): Promise<string> {
  const before = new Set((await sidebar.rows()).map((row) => row.id));
  await sidebar.click('[data-testid="new-session-button"]');
  await sidebar.clickWhenThere('[data-testid="kind-shell"]');
  await sidebar.click('[data-testid="create-session"]');
  const rows = await sidebar.waitForRows("the new session in the sidebar", (list) =>
    list.some((row) => !before.has(row.id) && row.active),
  );
  const created = rows.find((row) => !before.has(row.id));
  if (!created) throw new Error("no new session");
  await inFront(
    terminal,
    "the new session to run in front",
    (state) => state.session === created.id && state.status.kind === "running",
  );
  await terminal.waitForPrompt();
  return created.id;
}

/** The numbers of the `TICK-<id>:<n>` lines of the terminal in front, in order. */
export async function ticksOf(terminal: Terminal, id: string, tail = 1_000): Promise<number[]> {
  const prefix = `TICK-${id}:`;
  return (await terminal.lines(tail))
    .filter((line) => line.startsWith(prefix))
    .map((line) => Number(line.slice(prefix.length)));
}

/** 1, 2, ... `count`. */
export function upTo(count: number): number[] {
  return Array.from({ length: count }, (_, index) => index + 1);
}

/**
 * Prints how long each step of a spec took, to see where the time goes on
 * a slow machine.
 */
export function stepTimer(spec: string): (step: string) => void {
  let last = Date.now();
  return (step) => {
    const now = Date.now();
    console.info(`[${spec}] ${step}: ${String(now - last)} ms`);
    last = now;
  };
}
