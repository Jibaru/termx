import { describe, expect, it } from "vitest";

import styles from "../styles.css?raw";
import { DEFAULT_TERMINAL_THEME } from "../terminal/config";
import {
  applyPalette,
  contrast,
  cssVariable,
  DEFAULT_PALETTE,
  ICON_BACKGROUNDS,
  ICON_ROLES,
  type PaletteRole,
} from "./palette";

const ROLES = Object.keys(DEFAULT_PALETTE) as PaletteRole[];

describe("the palette", () => {
  it("is what styles.css paints with, role for role", () => {
    for (const role of ROLES) {
      expect(styles, `${cssVariable(role)} in styles.css`).toContain(
        `${cssVariable(role)}: ${DEFAULT_PALETTE[role]};`,
      );
    }
  });

  it("is opaque: every colour is #rrggbb", () => {
    for (const role of ROLES) expect(DEFAULT_PALETTE[role], role).toMatch(/^#[0-9a-f]{6}$/);
  });

  it.each(ICON_BACKGROUNDS)("draws every icon at 3:1 or more against --color-%s", (background) => {
    for (const role of ICON_ROLES) {
      expect(
        contrast(DEFAULT_PALETTE[role], DEFAULT_PALETTE[background]),
        `${role} on ${background}`,
      ).toBeGreaterThanOrEqual(3);
    }
  });

  it.each(["surface", "selected", "hover", "surfaceRaised"] as const)(
    "draws text at 4.5:1 or more against --color-%s",
    (background) => {
      for (const role of ["text", "textMuted"] as const) {
        expect(
          contrast(DEFAULT_PALETTE[role], DEFAULT_PALETTE[background]),
          `${role} on ${background}`,
        ).toBeGreaterThanOrEqual(4.5);
      }
    },
  );

  it("keeps the text of the loud states and of the buttons readable", () => {
    expect(contrast(DEFAULT_PALETTE.attentionText, DEFAULT_PALETTE.attention)).toBeGreaterThan(4.5);
    expect(contrast("#ffffff", DEFAULT_PALETTE.accentStrong)).toBeGreaterThan(4.5);
    expect(contrast("#ffffff", DEFAULT_PALETTE.dangerStrong)).toBeGreaterThan(4.5);
    expect(contrast(DEFAULT_PALETTE.warningText, DEFAULT_PALETTE.warningBg)).toBeGreaterThan(4.5);
    expect(contrast(DEFAULT_PALETTE.dangerText, DEFAULT_PALETTE.dangerBg)).toBeGreaterThan(4.5);
    expect(contrast(DEFAULT_PALETTE.text, DEFAULT_PALETTE.infoBg)).toBeGreaterThan(4.5);
  });

  it("gives each kind of session and each loud state a colour of its own", () => {
    const own = ["kindClaude", "kindShell", "kindBackground", "attention", "danger", "success"];
    const colours = own.map((role) => DEFAULT_PALETTE[role as PaletteRole]);
    expect(new Set(colours).size).toBe(own.length);
  });

  it("is shared with the terminal where the two overlap", () => {
    expect(DEFAULT_TERMINAL_THEME.background).toBe(DEFAULT_PALETTE.bg);
    expect(DEFAULT_TERMINAL_THEME.foreground).toBe(DEFAULT_PALETTE.text);
    expect(DEFAULT_TERMINAL_THEME.blue).toBe(DEFAULT_PALETTE.info);
    expect(DEFAULT_TERMINAL_THEME.brightBlue).toBe(DEFAULT_PALETTE.accent);
    expect(DEFAULT_TERMINAL_THEME.green).toBe(DEFAULT_PALETTE.success);
    expect(DEFAULT_TERMINAL_THEME.brightRed).toBe(DEFAULT_PALETTE.danger);
    expect(DEFAULT_TERMINAL_THEME.selectionBackground).toBe(DEFAULT_PALETTE.accentStrong);
  });

  it("can be overridden at run time, role by role", () => {
    const root = document.createElement("div");

    const applied = applyPalette({ accent: "#123456" }, root);

    expect(applied.accent).toBe("#123456");
    expect(root.style.getPropertyValue("--color-accent")).toBe("#123456");
    expect(root.style.getPropertyValue("--color-kind-claude")).toBe(DEFAULT_PALETTE.kindClaude);
  });
});
