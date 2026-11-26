import type { Platform } from "../lib/platform";

/**
 * The keys termx handles itself instead of leaving them to xterm.js.
 *
 * Everything else (Ctrl+C, Ctrl+D, Ctrl+Z, Escape, the arrows with their
 * modifiers, Option as Meta...) is encoded by xterm.js the way xterm does.
 */

/**
 * What Shift+Enter sends: `ESC` followed by a carriage return, which is
 * also what Option+Enter (Meta+Enter) sends.
 *
 * A terminal cannot tell Enter and Shift+Enter apart in the classic
 * encoding: both are a carriage return. Claude Code inserts a newline when
 * it receives this sequence, and it is the one its `/terminal-setup` command
 * binds to Shift+Enter in the terminals built on xterm.js (VS Code, Cursor).
 * zsh inserts a newline for it too. With the kitty keyboard protocol the
 * key would be reported as such, but no stable xterm.js release has it.
 */
export const SHIFT_ENTER_SEQUENCE = "\x1b\r";

/** What to do with a key event. */
export type KeyAction =
  /** Let xterm.js encode the key as usual. */
  | { kind: "default" }
  /** Send this to the process instead. */
  | { kind: "send"; data: string }
  /** Copy the selection to the clipboard. */
  | { kind: "copy" }
  /** Paste the clipboard into the terminal. */
  | { kind: "paste" };

/** The parts of a `KeyboardEvent` the key map looks at. */
export type KeyStroke = Pick<
  KeyboardEvent,
  "key" | "code" | "shiftKey" | "ctrlKey" | "altKey" | "metaKey"
>;

const DEFAULT: KeyAction = { kind: "default" };

/**
 * Decides what a key does.
 *
 * Copy and paste follow the convention of each platform:
 *
 * - **macOS**: Cmd+C and Cmd+V. Nothing is mapped here, because they are not
 *   seen as keys at all: the webview turns them into `copy` and `paste`
 *   events, which xterm.js handles. Ctrl+C stays the interrupt key.
 * - **Windows and Linux**: Ctrl+Shift+C and Ctrl+Shift+V, since Ctrl+C and
 *   Ctrl+V belong to the program in the terminal, plus the older Ctrl+Insert
 *   and Shift+Insert.
 */
export function mapKey(key: KeyStroke, platform: Platform): KeyAction {
  if (key.key === "Enter" && key.shiftKey && !key.ctrlKey && !key.altKey && !key.metaKey) {
    return { kind: "send", data: SHIFT_ENTER_SEQUENCE };
  }

  if (platform !== "macos" && !key.altKey && !key.metaKey) {
    if (key.ctrlKey && key.shiftKey) {
      // `code` is the physical key, so this holds whatever the layout or
      // the state of Caps Lock.
      if (key.code === "KeyC") return { kind: "copy" };
      if (key.code === "KeyV") return { kind: "paste" };
    }
    if (key.key === "Insert") {
      if (key.ctrlKey && !key.shiftKey) return { kind: "copy" };
      if (key.shiftKey && !key.ctrlKey) return { kind: "paste" };
    }
  }

  return DEFAULT;
}
