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
   * On macOS, whether the Option key acts as Meta (it sends `ESC` followed
   * by the key) instead of typing the alternate characters of the layout.
   * Claude Code and shells use Meta for shortcuts such as Option+Enter and
   * Option+B.
   */
  macOptionIsMeta: boolean;
  /**
   * Text whose contrast with its background is below this ratio is drawn
   * lighter or darker until it reaches it. 1 leaves colours alone; 4.5 is
   * the WCAG AA threshold.
   */
  minimumContrastRatio: number;
  /** Space between the terminal and the edges of its pane, in CSS pixels. */
  padding: { x: number; y: number };
  /**
   * `webgl` draws with the GPU and falls back to `dom` by itself when
   * WebGL is unavailable or its context is lost.
   */
  renderer: "webgl" | "dom";
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
 */
export const DEFAULT_TERMINAL_THEME: TerminalTheme = {
  background: "#0a0a0a",
  foreground: "#f5f5f5",
  cursor: "#f5f5f5",
  cursorAccent: "#0a0a0a",
  selectionBackground: "#005f87",
  selectionInactiveBackground: "#3a3a3a",
  // Lighter than the background, so that "black" text stays visible.
  black: "#262626",
  red: "#ff0000",
  green: "#5fd700",
  yellow: "#d7af00",
  blue: "#0087af",
  magenta: "#af5fd7",
  cyan: "#00afaf",
  white: "#d0d0d0",
  brightBlack: "#808080",
  brightRed: "#ff5f5f",
  brightGreen: "#87ff5f",
  brightYellow: "#ffd75f",
  brightBlue: "#00afff",
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
  macOptionIsMeta: true,
  minimumContrastRatio: 4.5,
  padding: { x: 6, y: 4 },
  renderer: "webgl",
  unicodeVersion: "15-graphemes",
  theme: DEFAULT_TERMINAL_THEME,
};
