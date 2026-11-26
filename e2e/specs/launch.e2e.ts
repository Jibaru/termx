import { describe, expect, it } from "vitest";

import { useApp } from "../src/fixture.ts";
import type { Drawing } from "../src/page.ts";
import { type Coverage, coverage } from "../src/pixels.ts";
import { captureScreen } from "../src/screen.ts";
import { waitFor } from "../src/wait.ts";

/**
 * Fewest pixels that must differ from the background once the terminal has
 * printed six rows of forty solid cells. Those cover several times this
 * many pixels with any font; an empty terminal has a cursor and no more.
 */
const MIN_DRAWN_PIXELS = 8_000;

/** Fewest characters the DOM renderer must hold: those of the marker. */
const MIN_DRAWN_CHARACTERS = 12;

describe("launching the application", () => {
  const fixture = useApp("launch");

  it("runs a shell in a terminal", async () => {
    const { terminal } = fixture;

    const state = await terminal.state();
    expect(state.status.kind).toBe("running");
    expect(state.cols).toBeGreaterThanOrEqual(20);
    expect(state.rows).toBeGreaterThanOrEqual(5);

    // The prompt of the shell the suite asks for, not of a login shell.
    await terminal.waitForLine("the prompt of the shell", (line) => line.startsWith("termx-e2e"));
  });

  it("draws what the shell prints", async () => {
    const { app, terminal } = fixture;
    const { id } = await terminal.startHelper("paint");
    await terminal.waitForExactLine(`PAINT-${id}`);

    const layout = await terminal.layout();
    expect(layout.screen.width).toBeGreaterThan(100);
    expect(layout.screen.height).toBeGreaterThan(50);

    // Drawing follows the buffer by a frame or more: look until it is there.
    // First where xterm.js draws...
    let drawing: Drawing | undefined;
    await waitFor(
      async () => {
        drawing = await terminal.drawing();
        return (
          drawing.drawn >= (drawing.renderer === "webgl" ? MIN_DRAWN_PIXELS : MIN_DRAWN_CHARACTERS)
        );
      },
      { what: "xterm.js to have drawn the terminal", describe: () => drawing },
    );

    // ...then on the screen, where a picture of it can be trusted.
    let onScreen: (Coverage & { of: string }) | undefined;
    await waitFor(
      async () => {
        const capture = await captureScreen(app.driver);
        if (!capture) return true;
        // A picture of the display has the window anywhere in it; the
        // rest of a virtual display is as dark as the background.
        const area = capture.of === "page" ? layout.terminal : undefined;
        onScreen = { ...coverage(capture.png, layout, area), of: capture.of };
        return onScreen.drawn >= MIN_DRAWN_PIXELS;
      },
      { what: "the terminal to be drawn on the screen", describe: () => onScreen },
    );

    console.info(
      `drawn by xterm.js: ${JSON.stringify(drawing)}; on screen: ${JSON.stringify(onScreen)}`,
    );
  });
});
