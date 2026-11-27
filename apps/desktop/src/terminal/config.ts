import { DEFAULT_PALETTE } from "../theme/palette";

/**
 * The look and the behaviour of the terminal view, in one place.
 *
 * Everything here is a default. The future `config.toml` will override these
 * values, which is why they are plain data: no value is read from anywhere
 * else by the terminal view.
 */

/** The colours of the terminal: its surfaces and the 16 ANSI colours. */
export interface TerminalTheme {
  background: string;
  foreground: string;
  cursor: string;
  /** Colour of the character under a block cursor. */
  cursorAccent: string;
  selectionBackground: string;
  /** Selection colour while the terminal does not have the focus. */
  selectionInactiveBackground: string;
  black: string;
  red: string;
  green: string;
  yellow: string;
  blue: string;
  magenta: string;
  cyan: string;
  white: string;
  brightBlack: string;
  brightRed: string;
  brightGreen: string;
  brightYellow: string;
  brightBlue: string;
  brightMagenta: string;
  brightCyan: string;
  brightWhite: string;
}

export interface TerminalConfig {
  /** CSS font stack. The first font that is installed is used. */
  fontFamily: string;
  /** Font size, in CSS pixels. */
  fontSize: number;
  /** Line height, as a multiple of the font's own. */
  lineHeight: number;
  /** Extra space between characters, in CSS pixels. */
  letterSpacing: number;
  /** Number of lines kept above the screen. */
  scrollback: number;
  cursorBlink: boolean;
  cursorStyle: "block" | "underline" | "bar";
  /**
   * On macOS, which Option keys act as Meta (they send `ESC` followed by
   * the key) instead of typing the alternate characters of the keyboard
   * layout. Claude Code and shells use Meta for shortcuts such as Option+B.
   *
   * - `left`: the left Option key is Meta, the right one types characters.
   *   Shortcuts work, and so do á, ñ, ¿ or, on layouts that need Option for
   *   them, @ # [ ] { } \ | ~.
   * - `both`: both keys are Meta. Those characters cannot be typed.
   * - `none`: both keys type characters. Meta shortcuts need Escape.
   *
   * Option+Enter inserts a new line in Claude Code under every setting.
   */
  optionAsMeta: "left" | "both" | "none";
  /**
   * Text whose contrast with its background is below this ratio is drawn
   * lighter or darker until it reaches it. 1 leaves colours alone; 4.5 is
   * the WCAG AA threshold.
   */
  minimumContrastRatio: number;
  /** Space between the terminal and the edges of its pane, in CSS pixels. */
  padding: { x: number; y: number };
  /**
   * What draws the terminal.
   *
   * - `auto`: WebGL, and the DOM renderer when WebGL is unavailable or its
   *   context is lost.
   * - `webgl`: WebGL, started again when its context is lost. The DOM
   *   renderer is only used when WebGL cannot be started at all.
   * - `dom`: never WebGL. Slower, but the one to choose if WebGL draws
   *   garbled text on a machine.
   */
  renderer: "auto" | "webgl" | "dom";
  /**
   * Whether programs may put text on the clipboard with the OSC 52 escape
   * sequence, as Claude Code's `/copy` and tmux do. Reading the clipboard
   * that way is never allowed, whatever this says.
   */
  allowOsc52Write: boolean;
  /**
   * How the width of characters is decided. `15-graphemes` treats an emoji
   * built from several code points as one wide character, as current
   * command-line programs expect; `15` and `6` are the older rules.
   */
  unicodeVersion: "15-graphemes" | "15" | "6";
  theme: TerminalTheme;
}

/**
 * Dark, neutral and high contrast, on an opaque near-black background.
 *
 * The ANSI colours are picked from the xterm 256-colour palette so that they
 * look the same as what programs get when they ask for those indices, and
 * each of them except black keeps a contrast of at least 4.5:1 against the
 * background: blue is colour 31 and bright blue 39, green is 76, red is 196.
 *
 * Where the terminal and the window use the same colour, it comes from the
 * palette of the window (`theme/palette.ts`): the background, the text, the
 * blues, the green and the bright red. They are the same colours as
 * before, now shared.
 */
export const DEFAULT_TERMINAL_THEME: TerminalTheme = {
  background: DEFAULT_PALETTE.bg,
  foreground: DEFAULT_PALETTE.text,
  cursor: DEFAULT_PALETTE.text,
  cursorAccent: DEFAULT_PALETTE.bg,
  selectionBackground: DEFAULT_PALETTE.accentStrong,
  selectionInactiveBackground: "#3a3a3a",
  // Lighter than the background, so that "black" text stays visible.
  black: "#262626",
  red: "#ff0000",
  green: DEFAULT_PALETTE.success,
  yellow: "#d7af00",
  blue: DEFAULT_PALETTE.info,
  magenta: "#af5fd7",
  cyan: "#00afaf",
  white: "#d0d0d0",
  brightBlack: "#808080",
  brightRed: DEFAULT_PALETTE.danger,
  brightGreen: "#87ff5f",
  brightYellow: "#ffd75f",
  brightBlue: DEFAULT_PALETTE.accent,
  brightMagenta: "#d787ff",
  brightCyan: "#5fd7d7",
  brightWhite: "#ffffff",
};

export const DEFAULT_TERMINAL_CONFIG: TerminalConfig = {
  fontFamily: '"MesloLGS NF", "Fira Code", Menlo, Consolas, "DejaVu Sans Mono", monospace',
  fontSize: 14,
  lineHeight: 1,
  letterSpacing: 0,
  scrollback: 10_000,
  cursorBlink: false,
  cursorStyle: "block",
  optionAsMeta: "left",
  minimumContrastRatio: 4.5,
  padding: { x: 6, y: 4 },
  renderer: "auto",
  allowOsc52Write: true,
  unicodeVersion: "15-graphemes",
  theme: DEFAULT_TERMINAL_THEME,
};
