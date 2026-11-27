import { describe, expect, it } from "vitest";

import { useApp } from "../src/fixture.ts";
import { inFront, newShell, stepTimer, ticksOf, upTo } from "../src/sessions.ts";
import { Sidebar } from "../src/sidebar.ts";

const TICKS = 150;
const TICK_MS = 20;

describe("switching between sessions", () => {
  const fixture = useApp("switching");

  it("keeps the scrollback of each, with no line lost or repeated", async () => {
    const { app, terminal } = fixture;
    const step = stepTimer("switching");
    const sidebar = new Sidebar(app.driver);
    const first = (await terminal.state()).session;
    const one = await terminal.startHelper("ticks", String(TICKS), String(TICK_MS));
    await terminal.waitForLine("the first program", (line) => line.startsWith(`TICK-${one.id}:`));
    const second = await newShell(sidebar, terminal);
    const two = await terminal.startHelper("ticks", String(TICKS), String(TICK_MS));
    await terminal.waitForLine("the second program", (line) => line.startsWith(`TICK-${two.id}:`));
    step("both programs printing");

    // Back and forth while both print: each shown in turn, the other hidden.
    for (let round = 0; round < 6; round += 1) {
      const target = round % 2 === 0 ? first : second;
      await sidebar.clickRow(target);
      await inFront(
        terminal,
        "the session clicked to be in front",
        (state) => state.session === target,
      );
    }
    step("switched six times");

    for (const [session, run] of [
      [first, one],
      [second, two],
    ] as const) {
      await sidebar.clickRow(session);
      await inFront(terminal, "the session to be in front", (state) => state.session === session);
      await terminal.waitForExactLine(`TICKS-DONE-${run.id}`, 60_000);
      expect(await ticksOf(terminal, run.id)).toEqual(upTo(TICKS));
    }
    step("both scrollbacks checked");
  });
});
