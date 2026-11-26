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
  "key" | "code" | "shiftKey" | "ctrlKey" | "altKey" | "metaKey" | "isComposing"
> & {
  /**
   * The legacy key code. Deprecated everywhere else, but still the only
   * thing that marks the keys of an input method in every webview: they all
   * carry 229, also when `isComposing` is not set yet.
   */
  readonly keyCode: number;
};

const DEFAULT: KeyAction = { kind: "default" };

/** The `keyCode` of every key event that belongs to an input method. */
const IME_KEY_CODE = 229;

/**
 * Whether the key is part of a composition: an input method (Chinese,
 * Japanese, Korean...) or a dead key is building a character out of several
 * keys. Such keys belong to the input method, Enter included, which
 * confirms the text being composed.
 */
export function isComposing(key: Pick<KeyStroke, "isComposing" | "keyCode">): boolean {
  return key.isComposing || key.keyCode === IME_KEY_CODE;
}

/**
 * Decides what a key does.
 *
 * Nothing is ever intercepted during a composition.
 *
 * Shift+Enter and Option+Enter (Alt+Enter) both send
 * [`SHIFT_ENTER_SEQUENCE`], whatever the Option key is set to do.
 *
 * Copy and paste follow the convention of each platform:
 *
 * - **macOS**: Cmd+C and Cmd+V. Nothing is mapped here, because they are not
 *   seen as keys at all: the webview turns them into `copy` and `paste`
 *   events. Ctrl+C stays the interrupt key.
 * - **Windows and Linux**: Ctrl+Shift+C and Ctrl+Shift+V, since Ctrl+C and
 *   Ctrl+V belong to the program in the terminal, plus the older Ctrl+Insert
 *   and Shift+Insert.
 */
export function mapKey(key: KeyStroke, platform: Platform): KeyAction {
  if (isComposing(key)) return DEFAULT;

  if (key.key === "Enter" && (key.shiftKey || key.altKey) && !key.ctrlKey && !key.metaKey) {
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

/** Which Option keys act as Meta on macOS. See `TerminalConfig`. */
export type OptionAsMeta = "left" | "both" | "none";

/**
 * Follows the two Option keys of a Mac keyboard.
 *
 * A key event says that Option is held, not which one. The keys themselves
 * do: each reports going down and up with its own `code`. Feed every key
 * event to `track`, then ask `actsAsMeta` about the key at hand.
 */
export class OptionKeys {
  private left = false;
  private right = false;

  track(event: Pick<KeyboardEvent, "type" | "code" | "altKey">): void {
    const down = event.type === "keydown";
    if (event.code === "AltLeft") {
      this.left = down;
    } else if (event.code === "AltRight") {
      this.right = down;
    } else if (!event.altKey) {
      // Both were released while nobody was looking, for instance in
      // another window.
      this.reset();
    }
  }

  /** Forgets the keys, when their release may have been missed. */
  reset(): void {
    this.left = false;
    this.right = false;
  }

  /**
   * Whether Option acts as Meta for a key pressed now. With `left`, the
   * right Option key wins when both are held, and an Option key that was
   * not seen going down counts as the right one: typing a character is the
   * safe reading of a key termx knows nothing about.
   */
  actsAsMeta(mode: OptionAsMeta): boolean {
    switch (mode) {
      case "both":
        return true;
      case "none":
        return false;
      case "left":
        return this.left && !this.right;
    }
  }
}
