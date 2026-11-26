import { describe, expect, it } from "vitest";

import { DEFAULT_TERMINAL_CONFIG, DEFAULT_TERMINAL_THEME } from "./config";

/** Relative luminance of a `#rrggbb` colour, as defined by WCAG. */
function luminance(hex: string): number {
  const channels = [1, 3, 5].map((start) => parseInt(hex.slice(start, start + 2), 16) / 255);
  const [red = 0, green = 0, blue = 0] = channels.map((channel) =>
    channel <= 0.03928 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4,
  );
  return 0.2126 * red + 0.7152 * green + 0.0722 * blue;
}

function contrast(first: string, second: string): number {
  const [lighter, darker] = [luminance(first), luminance(second)].sort((a, b) => b - a) as [
    number,
    number,
  ];
  return (lighter + 0.05) / (darker + 0.05);
}

describe("the default terminal configuration", () => {
  it("uses the documented font stack, size and scrollback", () => {
    expect(DEFAULT_TERMINAL_CONFIG.fontFamily).toBe(
      '"MesloLGS NF", "Fira Code", Menlo, Consolas, "DejaVu Sans Mono", monospace',
    );
    expect(DEFAULT_TERMINAL_CONFIG.fontSize).toBe(14);
    expect(DEFAULT_TERMINAL_CONFIG.scrollback).toBe(10_000);
    expect(DEFAULT_TERMINAL_CONFIG.macOptionIsMeta).toBe(true);
  });

  it("is near-white on an opaque near-black background", () => {
    const { background, foreground } = DEFAULT_TERMINAL_THEME;

    expect(background).toMatch(/^#[0-9a-f]{6}$/);
    expect(luminance(background)).toBeLessThan(0.01);
    expect(contrast(foreground, background)).toBeGreaterThan(15);
  });

  it("keeps every ANSI colour except black readable on the background", () => {
    const { background, ...theme } = DEFAULT_TERMINAL_THEME;
    const ansi = Object.entries(theme).filter(
      ([name]) => !["black", "cursorAccent"].includes(name) && !name.startsWith("selection"),
    );

    expect(ansi).toHaveLength(17);
    for (const [name, colour] of ansi) {
      expect(contrast(colour, background), name).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("uses the xterm-256 accents: blues 31 and 39, green 76, red 196", () => {
    expect(DEFAULT_TERMINAL_THEME).toMatchObject({
      blue: "#0087af",
      brightBlue: "#00afff",
      green: "#5fd700",
      red: "#ff0000",
    });
  });
});
