import { describe, expect, it } from "vitest";

import { useApp } from "../src/fixture.ts";
import { isAlive } from "../src/processes.ts";
import { inFront, stepTimer, ticksOf, upTo } from "../src/sessions.ts";
import { DEFAULT_TIMEOUT_MS } from "../src/wait.ts";

/**
 * How many ticks the program prints, one every `TICK_MS`: 12 s, long
 * enough for the page to reload in between on a slow machine.
 */
const TICKS = 300;
const TICK_MS = 40;

describe("reloading the page", () => {
  const fixture = useApp("reload");

  it("keeps the session running, with its process and its whole output", async () => {
    const { app, terminal } = fixture;
    const step = stepTimer("reload");
    const before = await terminal.state();
    const { id } = await terminal.startHelper("ticks", String(TICKS), String(TICK_MS));
    const pid = await terminal.waitForLine("the process identifier of the program", (line) => {
      const match = new RegExp(`^TICKS-${id}:self=(\\d+)$`).exec(line);
      return match ? Number(match[1]) : undefined;
    });
    await terminal.waitForLine("a few ticks", (line) => line === `TICK-${id}:3`);
    step("program running");

    // What a reload of the webview, or its crash and return, does. From a
    // timer, so that the script returns before its page goes away: a
    // script that reloads its own page gets no answer to WebDriver, which
    // then waits for its own timeout.
    await app.driver.execute(
      "window.__TERMX_OLD_PAGE__ = true; setTimeout(() => location.reload(), 50); return true;",
    );
    // The old page is gone once its mark is: only then is what is in front
    // the new page's. Asked with the short timeout of `waitForPage`: a
    // question that reaches the old page as it goes away is never answered,
    // and waiting 15 s for it outlasted the program, which ends after 12 s.
    await app.driver.waitForPage(
      DEFAULT_TIMEOUT_MS,
      "return document.readyState === 'complete' && !window.__TERMX_OLD_PAGE__;",
    );

    const after = await inFront(
      terminal,
      "the same session to be in front again, running",
      (state) => state.session === before.session && state.status.kind === "running",
    );
    step("attached again");
    // The same terminal and the same process: nothing was started again.
    expect(after.status.terminal).toBe(before.status.terminal);
    expect(isAlive(pid)).toBe(true);

    // The output goes on, after a repaint of what came before the reload:
    // every tick once, in order, none lost in between.
    await terminal.waitForExactLine(`TICKS-DONE-${id}`, 60_000);
    expect(await ticksOf(terminal, id)).toEqual(upTo(TICKS));
    step("program done");

    // The shell is still the one from before, at its prompt, and takes input.
    await terminal.waitForPrompt();
    const echo = terminal.token("after");
    await terminal.submit(`echo ${echo}`);
    await terminal.waitForExactLine(echo);
    expect((await terminal.state()).status.terminal).toBe(before.status.terminal);
  });
});
