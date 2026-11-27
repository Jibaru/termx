/**
 * The colours of termx, by role: one palette for the window and the
 * terminal, so that the two look like one product.
 *
 * Dark, neutral and high contrast, on an opaque near-black background, with
 * a few saturated accents taken from the xterm 256-colour palette, which is
 * what programs in the terminal draw with: blue 31 and 39, green 76, red 203
 * (the bright red of the terminal), amber 214, magenta 207.
 *
 * - The CSS custom properties of `styles.css` (`--color-<role>`) hold these
 *   values, for the first paint; `palette.test.ts` keeps the two equal.
 * - The terminal takes its blues, green and bright red from here
 *   (`terminal/config.ts`).
 * - `applyPalette` overrides any of them at run time: that is where the
 *   future `config.toml` plugs in.
 * - Every colour an icon or a mark is drawn in keeps 3:1 or more against the
 *   sidebar, a selected row and a hovered row, and text 4.5:1; the test
 *   computes it, so an edit cannot break it unnoticed.
 */

export interface Palette {
  /** Behind everything: the main area and the terminal. */
  bg: string;
  /** The sidebar and the pane headers. */
  surface: string;
  /** Menus and dialogs. */
  surfaceRaised: string;
  /** A row under the pointer. */
  hover: string;
  /** The row of the session in the focused pane. */
  selected: string;
  border: string;
  borderStrong: string;
  /** Marks that carry meaning without a colour of their own. */
  indicator: string;
  text: string;
  textMuted: string;

  /** Links, focus, selection, the primary button. */
  accent: string;
  /** Behind white text: the primary button. */
  accentStrong: string;
  /** Something is on its way: starting, working. */
  info: string;
  /** It went well: running, exited cleanly, done. */
  success: string;
  /** It went wrong: an error code, a failure. */
  danger: string;
  /** Behind white text: the destructive button. */
  dangerStrong: string;
  /** It runs, but something is off. */
  warning: string;
  /** It cannot go on without the user: the loudest colour. */
  attention: string;
  /** Text on `attention`. */
  attentionText: string;
  /** Nothing to report: dormant, unknown. */
  muted: string;

  kindClaude: string;
  kindShell: string;
  kindBackground: string;

  /** The banners, opaque. */
  infoBg: string;
  warningBg: string;
  warningText: string;
  dangerBg: string;
  dangerText: string;
}

export const DEFAULT_PALETTE: Palette = {
  bg: "#0a0a0a",
  surface: "#111111",
  surfaceRaised: "#1a1a1a",
  hover: "#1c1c1c",
  selected: "#16273d",
  border: "#2a2a2a",
  borderStrong: "#404040",
  indicator: "#737373",
  text: "#f5f5f5",
  textMuted: "#a3a3a3",

  // xterm 39, and 24, the selection colour of the terminal.
  accent: "#00afff",
  accentStrong: "#005f87",
  // xterm 31: the blue of the terminal.
  info: "#0087af",
  // xterm 76: the green of the terminal.
  success: "#5fd700",
  // xterm 203: the bright red of the terminal; 160 behind white text.
  danger: "#ff5f5f",
  dangerStrong: "#d70000",
  // xterm 214.
  warning: "#ffaf00",
  // xterm 207: no other state uses it, so that nothing is mistaken for it.
  attention: "#ff5fff",
  attentionText: "#0a0a0a",
  // xterm 245.
  muted: "#8a8a8a",

  // xterm 215, 249 and 141.
  kindClaude: "#ffaf5f",
  kindShell: "#b2b2b2",
  kindBackground: "#af87ff",

  infoBg: "#10213a",
  warningBg: "#3a2a00",
  warningText: "#ffe2a0",
  dangerBg: "#4a0f0f",
  dangerText: "#ffd2d2",
};

export type PaletteRole = keyof Palette;

/** The CSS custom property of a role: `kindClaude` is `--color-kind-claude`. */
export function cssVariable(role: PaletteRole): string {
  return `--color-${role.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`)}`;
}

/**
 * Sets the palette on `target` (the root element by default), with
 * `overrides` on top of the default one. Returns the palette applied.
 */
export function applyPalette(
  overrides: Partial<Palette> = {},
  target: HTMLElement = document.documentElement,
): Palette {
  const palette = { ...DEFAULT_PALETTE, ...overrides };
  for (const role of Object.keys(palette) as PaletteRole[]) {
    target.style.setProperty(cssVariable(role), palette[role]);
  }
  return palette;
}

/** Relative luminance of a `#rrggbb` colour, as WCAG defines it. */
export function luminance(hex: string): number {
  const channels = [1, 3, 5].map((start) => Number.parseInt(hex.slice(start, start + 2), 16) / 255);
  const [r = 0, g = 0, b = 0] = channels.map((c) =>
    c <= 0.039_28 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4,
  );
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** The WCAG contrast ratio of two `#rrggbb` colours, from 1 to 21. */
export function contrast(a: string, b: string): number {
  const [light = 0, dark = 0] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (light + 0.05) / (dark + 0.05);
}

/** The colours icons and marks are drawn in. */
export const ICON_ROLES: readonly PaletteRole[] = [
  "indicator",
  "accent",
  "info",
  "success",
  "danger",
  "warning",
  "attention",
  "muted",
  "kindClaude",
  "kindShell",
  "kindBackground",
];

/** The backgrounds icons are drawn on. */
export const ICON_BACKGROUNDS: readonly PaletteRole[] = ["surface", "selected", "hover", "bg"];
