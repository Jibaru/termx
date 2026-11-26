import { describe, expect, it } from "vitest";

import { useApp } from "../src/fixture.ts";
import { chord } from "../src/keys.ts";
import { hexOf } from "../src/terminal.ts";
import { waitFor } from "../src/wait.ts";

const PASTE_START = hexOf("\x1b[200~");
const PASTE_END = hexOf("\x1b[201~");

describe("pasting into the terminal", () => {
  const fixture = useApp("paste");

  it("wraps the text in the bracketed paste markers when the program asked for them", async () => {
    const raw = await fixture.terminal.startRaw({ bracketedPaste: true });

    await fixture.terminal.paste("echo pasted");

    await raw.waitForBytes(`${PASTE_START} ${hexOf("echo pasted")} ${PASTE_END}`);
    await raw.stop();
  });

  it("removes an end marker hidden in the pasted text", async () => {
    const raw = await fixture.terminal.startRaw({ bracketedPaste: true });

    // Without the removal, what follows the marker would be typed input.
    await fixture.terminal.paste("safe\x1b[201~echo unsafe\r");

    await raw.waitForBytes(`${PASTE_START} ${hexOf("safeecho unsafe\r")} ${PASTE_END}`);
    await raw.stop();
  });

  it("removes an end marker that only appears once another one is removed", async () => {
    const raw = await fixture.terminal.startRaw({ bracketedPaste: true });

    await fixture.terminal.paste("a\x1b[20\x1b[201~1~b");

    await raw.waitForBytes(`${PASTE_START} ${hexOf("ab")} ${PASTE_END}`);
    await raw.stop();
  });

  it("pastes the text alone, with carriage returns, when the program did not ask", async () => {
    const raw = await fixture.terminal.startRaw();

    await fixture.terminal.paste("one\ntwo\r\nthree");

    await raw.waitForBytes(hexOf("one\rtwo\rthree"));
    await raw.stop();
  });

  // Ctrl+Shift+V reads the clipboard of the system, which this test
  // therefore overwrites: it only runs where that is fine. macOS pastes
  // with a `paste` event instead, which the tests above deliver.
  const usesTheClipboard =
    process.platform !== "darwin" &&
    (process.env.CI !== undefined || process.env.TERMX_E2E_CLIPBOARD === "1");

  it.runIf(usesTheClipboard)(
    "pastes what a program copied with OSC 52 when Ctrl+Shift+V is pressed",
    async () => {
      const { terminal } = fixture;
      const text = `copied-${terminal.token()}`;
      const { id } = await terminal.startHelper("copy", hexOf(text));
      await terminal.waitForExactLine(`COPY-${id}`);
      // The program asked; the application writes the clipboard on its own time.
      let clipboard = "";
      await waitFor(
        async () => {
          clipboard = await terminal.readClipboard();
          return clipboard === text;
        },
        { what: "the clipboard to hold the copied text", describe: () => clipboard },
      );
      const raw = await terminal.startRaw({ bracketedPaste: true });

      await terminal.keys(
        chord({ key: "V", code: "KeyV", keyCode: 86 }, { ctrl: true, shift: true }),
      );

      await raw.waitForBytes(`${PASTE_START} ${hexOf(text)} ${PASTE_END}`);
      await raw.stop();
      expect(await terminal.readClipboard()).toBe(text);
    },
  );
});
