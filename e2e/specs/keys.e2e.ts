import { describe, it } from "vitest";

import { useApp } from "../src/fixture.ts";
import { enter, option } from "../src/keys.ts";

/**
 * What the terminal sends for the keys termx maps itself, seen by a program
 * that prints the bytes it receives.
 *
 * The keys are `KeyboardEvent`s delivered to the terminal, described the
 * way the webview of each platform describes the real key. That covers
 * everything from the key handler of termx to the program: the key map,
 * xterm.js, the IPC, the PTY. It does not cover the webview turning a
 * physical key into that event.
 */
describe("the keys termx maps", () => {
  const fixture = useApp("keys");

  /** `ESC` and a carriage return: what Claude Code reads as "new line". */
  const NEW_LINE = "1b 0d";

  it("sends a carriage return for Enter", async () => {
    const raw = await fixture.terminal.startRaw();

    await fixture.terminal.keys(enter());

    await raw.waitForBytes("0d");
    await raw.stop();
  });

  it("sends the new line sequence for Shift+Enter", async () => {
    const raw = await fixture.terminal.startRaw();

    await fixture.terminal.keys(enter({ shift: true }));

    await raw.waitForBytes(NEW_LINE);
    await raw.stop();
  });

  it.each(["left", "right"] as const)(
    "sends the new line sequence for Alt+Enter with the %s Alt key",
    async (side) => {
      const raw = await fixture.terminal.startRaw();

      await fixture.terminal.keys(enter({ alt: true, side }));

      await raw.waitForBytes(NEW_LINE);
      await raw.stop();
    },
  );

  it("sends each key once when Enter follows Shift+Enter", async () => {
    const raw = await fixture.terminal.startRaw();

    await fixture.terminal.keys([...enter({ shift: true }), ...enter()]);

    await raw.waitForBytes(`${NEW_LINE} 0d`);
    await raw.stop();
  });

  // The two Option keys only differ on macOS: `optionAsMeta` is "left".
  describe.runIf(process.platform === "darwin")("the Option keys of a Mac", () => {
    it("types the character of the layout with the right Option key", async () => {
      const raw = await fixture.terminal.startRaw();

      // Option+A is "å" on a US keyboard.
      await fixture.terminal.keys(option("a", "å", "right"));

      await raw.waitForBytes("c3 a5");
      await raw.stop();
    });

    it("sends ESC and the key with the left Option key", async () => {
      const raw = await fixture.terminal.startRaw();

      // Option+B is "∫" on a US keyboard, and "back one word" as Meta+B.
      await fixture.terminal.keys(option("b", "∫", "left"));

      await raw.waitForBytes("1b 62");
      await raw.stop();
    });

    it("decides for each key, when both Option keys are used in turn", async () => {
      const raw = await fixture.terminal.startRaw();

      await fixture.terminal.keys([
        ...option("a", "å", "left"),
        ...option("a", "å", "right"),
        ...option("a", "å", "left"),
      ]);

      await raw.waitForBytes("1b 61 c3 a5 1b 61");
      await raw.stop();
    });
  });
});
