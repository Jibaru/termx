import { describe, expect, it } from "vitest";

import { useApp } from "../src/fixture.ts";
import { hexOf } from "../src/terminal.ts";
import { waitFor } from "../src/wait.ts";

/** Accents, wide characters, a symbol, and emoji made of one and of several code points. */
const UNICODE_TEXT = "héllo ñandú 日本語 ✓ 👍 👍🏽 👨‍👩‍👧";

/** How long the page may take to answer while output pours in, in milliseconds. */
const MAX_PAGE_ANSWER_MS = 10_000;

/** How long a flood may keep printing after Ctrl+C, in milliseconds. */
const MAX_INTERRUPT_MS = 10_000;

describe("output and input that are not plain ASCII", () => {
  const fixture = useApp("unicode");

  it("shows UTF-8 text and emoji as the program printed them", async () => {
    const { terminal } = fixture;

    const { id } = await terminal.startHelper("text", hexOf(UNICODE_TEXT));

    await terminal.waitForExactLine(`TEXT-${id}:${UNICODE_TEXT}`);
  });

  it("sends pasted UTF-8 text and emoji to the program byte for byte", async () => {
    const raw = await fixture.terminal.startRaw();

    await fixture.terminal.paste(UNICODE_TEXT);

    await raw.waitForBytes(hexOf(UNICODE_TEXT));
    await raw.stop();
  });
});

describe("a program that prints a lot", () => {
  const fixture = useApp("flood");

  it("gets several megabytes through without freezing the window", async () => {
    const { terminal } = fixture;
    const megabytes = 4;

    const { id } = await terminal.startHelper("burst", String(megabytes));

    // While the output pours in, the page must keep answering.
    let slowestAnswer = 0;
    const lineCount = await waitFor(
      async () => {
        slowestAnswer = Math.max(slowestAnswer, await terminal.ping());
        const lines = await terminal.lines(5);
        const end = lines.find((line) => line.startsWith(`BURST-${id}:lines=`));
        return end === undefined ? undefined : Number(end.split("=")[1]);
      },
      { what: "the end of the burst", timeoutMs: 180_000 },
    );
    expect(slowestAnswer).toBeLessThan(MAX_PAGE_ANSWER_MS);

    // Nothing was lost on the way: the last numbered line is right above.
    const tail = await terminal.lines(5);
    const lastLine = tail[tail.findIndex((line) => line.startsWith(`BURST-${id}:`)) - 1];
    expect(lastLine).toMatch(new RegExp(`^burst ${String(lineCount - 1).padStart(8, "0")} x+$`));
    expect(lineCount).toBeGreaterThan((megabytes * 1024 * 1024) / 100);

    // And the terminal still works.
    await terminal.waitForPrompt();
    console.info(
      `burst: ${String(lineCount)} lines; slowest answer of the page: ${slowestAnswer.toFixed(0)} ms`,
    );
  });

  it("stops a flood promptly when Ctrl+C is pressed", async () => {
    const { terminal } = fixture;
    const { id } = await terminal.startHelper("flood");
    const flooding = new RegExp(`^flood-${id} (\\d+)$`);
    await terminal.waitForLine(
      "the flood to be well under way",
      (line) => Number(flooding.exec(line)?.[1] ?? 0) > 20_000,
      { tail: 5, timeoutMs: 120_000 },
    );

    const pressed = performance.now();
    await terminal.interrupt();
    // The shell is back, and what was typed after the flood is run.
    const token = terminal.token("after");
    await terminal.submit(`echo ${token}`);
    await terminal.waitForExactLine(token, 60_000);
    const elapsed = performance.now() - pressed;

    console.info(`flood: stopped ${elapsed.toFixed(0)} ms after Ctrl+C`);
    expect(elapsed).toBeLessThan(MAX_INTERRUPT_MS);
  });
});
