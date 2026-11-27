import { describe, expect, it } from "vitest";

import styles from "./styles.css?raw";

/** The value of a colour token of `:root`. */
function token(name: string): string {
  const match = new RegExp(`--${name}:\\s*(#[0-9a-fA-F]{6});`).exec(styles);
  if (!match?.[1]) throw new Error(`no colour token --${name}`);
  return match[1];
}

/** Relative luminance, as WCAG defines it. */
function luminance(hex: string): number {
  const channels = [1, 3, 5].map((start) => Number.parseInt(hex.slice(start, start + 2), 16) / 255);
  const [r = 0, g = 0, b = 0] = channels.map((c) =>
    c <= 0.039_28 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4,
  );
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: string, b: string): number {
  const [light, dark] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return ((light ?? 0) + 0.05) / ((dark ?? 0) + 0.05);
}

describe("the colours of the interface", () => {
  it.each(["surface", "hover", "bg"])(
    "draw the marks that carry meaning at 3:1 or more against --color-%s",
    (surface) => {
      for (const mark of ["indicator", "accent", "ok", "warn", "error", "attention-bg"]) {
        expect(
          contrast(token(`color-${mark}`), token(`color-${surface}`)),
          `--color-${mark}`,
        ).toBeGreaterThanOrEqual(3);
      }
    },
  );

  it.each(["surface", "selected", "surface-raised"])(
    "draw text at 4.5:1 or more against --color-%s",
    (surface) => {
      for (const text of ["text", "text-muted"]) {
        expect(
          contrast(token(`color-${text}`), token(`color-${surface}`)),
          `--color-${text}`,
        ).toBeGreaterThanOrEqual(4.5);
      }
    },
  );
});
