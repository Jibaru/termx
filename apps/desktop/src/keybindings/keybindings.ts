import type { Platform } from "../lib/platform";

/**
 * The keyboard shortcuts of the window, in one place.
 *
 * Every binding is plain data, so that the future `config.toml` can replace
 * any of them: `resolveBindings` takes the overrides. Nothing else in the
 * frontend decides which key does what.
 *
 * Conventions:
 *
 * - **macOS**: Cmd, which no program in a terminal ever receives.
 * - **Windows and Linux**: Ctrl+Shift, as in the terminals of those systems.
 *   Ctrl alone, Alt alone and plain keys belong to the program in the
 *   terminal: Ctrl+C, Ctrl+D, Ctrl+Z, Ctrl+R, Ctrl+L, Ctrl+B... and the
 *   shortcuts of Claude Code. `PTY_RESERVED` lists them, and a test checks
 *   that no binding takes one.
 *
 * Some bindings only apply while the focus is outside the terminal (scope
 * `outside-terminal`): F2 renames the session selected in the sidebar, but
 * in a terminal F2 belongs to the program (htop, Midnight Commander...).
 */

/** What a shortcut does. */
export type CommandId =
  | "newSession"
  | "newSiblingSession"
  | "closeSession"
  | "splitRight"
  | "splitDown"
  | "closePane"
  | "focusPaneLeft"
  | "focusPaneRight"
  | "focusPaneUp"
  | "focusPaneDown"
  | "nextSession"
  | "previousSession"
  | "jumpToSession1"
  | "jumpToSession2"
  | "jumpToSession3"
  | "jumpToSession4"
  | "jumpToSession5"
  | "jumpToSession6"
  | "jumpToSession7"
  | "jumpToSession8"
  | "jumpToSession9"
  | "toggleSidebar"
  | "focusSidebar"
  | "renameSession";

/** Where a binding applies. */
export type BindingScope = "global" | "outside-terminal";

/**
 * A key with its modifiers. `code` is the physical key (`KeyT`, `Digit1`,
 * `BracketRight`, `ArrowLeft`, `F2`), so a binding holds whatever the
 * keyboard layout and Shift do to the character.
 */
export interface KeyChord {
  code: string;
  meta?: boolean;
  ctrl?: boolean;
  shift?: boolean;
  alt?: boolean;
}

export interface Binding {
  command: CommandId;
  chord: KeyChord;
  scope: BindingScope;
}

/** What the user can see about a command: its name, for menus and tooltips. */
export const COMMAND_LABELS: Record<CommandId, string> = {
  newSession: "New session…",
  newSiblingSession: "New Claude session in the same project",
  closeSession: "Close session",
  splitRight: "Split right",
  splitDown: "Split down",
  closePane: "Close pane",
  focusPaneLeft: "Focus the pane on the left",
  focusPaneRight: "Focus the pane on the right",
  focusPaneUp: "Focus the pane above",
  focusPaneDown: "Focus the pane below",
  nextSession: "Next session",
  previousSession: "Previous session",
  jumpToSession1: "Go to session 1",
  jumpToSession2: "Go to session 2",
  jumpToSession3: "Go to session 3",
  jumpToSession4: "Go to session 4",
  jumpToSession5: "Go to session 5",
  jumpToSession6: "Go to session 6",
  jumpToSession7: "Go to session 7",
  jumpToSession8: "Go to session 8",
  jumpToSession9: "Go to session 9",
  toggleSidebar: "Show or hide the sidebar",
  focusSidebar: "Go to the sidebar",
  renameSession: "Rename session",
};

const JUMPS = [1, 2, 3, 4, 5, 6, 7, 8, 9] as const;

function jumpCommand(n: (typeof JUMPS)[number]): CommandId {
  return `jumpToSession${String(n)}` as CommandId;
}

/** The bindings termx ships with on `platform`. */
export function defaultBindings(platform: Platform): Binding[] {
  const mac = platform === "macos";
  // The modifier of every shortcut: Cmd on macOS, Ctrl+Shift elsewhere.
  const primary: Omit<KeyChord, "code"> = mac ? { meta: true } : { ctrl: true, shift: true };
  const chord = (code: string, extra: Omit<KeyChord, "code"> = {}): KeyChord => ({
    code,
    ...primary,
    ...extra,
  });
  const global = (command: CommandId, key: KeyChord): Binding => ({
    command,
    chord: key,
    scope: "global",
  });

  return [
    global("newSession", chord("KeyT", mac ? { shift: true } : {})),
    global("newSiblingSession", chord("KeyN", mac ? { shift: true } : {})),
    global("closeSession", chord("KeyW", mac ? { shift: true } : {})),
    global("splitRight", chord("KeyD")),
    global("splitDown", mac ? chord("KeyD", { shift: true }) : chord("KeyE")),
    // Cmd+W on macOS closes a pane, never the window (see `src-tauri/src/quit.rs`).
    global("closePane", mac ? { code: "KeyW", meta: true } : chord("KeyW", { alt: true })),
    global("focusPaneLeft", focusChord(mac, "ArrowLeft")),
    global("focusPaneRight", focusChord(mac, "ArrowRight")),
    global("focusPaneUp", focusChord(mac, "ArrowUp")),
    global("focusPaneDown", focusChord(mac, "ArrowDown")),
    global("nextSession", chord("BracketRight", mac ? { shift: true } : {})),
    global("previousSession", chord("BracketLeft", mac ? { shift: true } : {})),
    ...JUMPS.map((n) => global(jumpCommand(n), chord(`Digit${String(n)}`))),
    // Ctrl+B is the readline and tmux key: elsewhere it is Ctrl+Shift+B.
    global("toggleSidebar", chord("KeyB")),
    global("focusSidebar", chord("Digit0")),
    { command: "renameSession", chord: { code: "F2" }, scope: "outside-terminal" },
  ];
}

/**
 * Cmd+Option+arrow on macOS, Ctrl+Shift+arrow elsewhere. Not Ctrl+Alt+arrow:
 * GNOME switches workspaces with it, and Intel graphics drivers on Windows
 * rotate the screen, before termx ever sees the key.
 */
function focusChord(mac: boolean, code: string): KeyChord {
  return mac ? { code, meta: true, alt: true } : { code, ctrl: true, shift: true };
}

/**
 * The bindings of `platform` with the user's `overrides` applied: a chord
 * given for a command replaces its default, and `null` removes it.
 */
export function resolveBindings(
  platform: Platform,
  overrides: Partial<Record<CommandId, KeyChord | null>> = {},
): Binding[] {
  const result: Binding[] = [];
  for (const binding of defaultBindings(platform)) {
    if (!(binding.command in overrides)) {
      result.push(binding);
      continue;
    }
    const chord = overrides[binding.command];
    if (chord) result.push({ ...binding, chord });
  }
  return result;
}

/** The parts of a key event a binding looks at. */
export type KeyEventLike = Pick<
  KeyboardEvent,
  "code" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey" | "isComposing"
> & { keyCode?: number };

export function chordMatches(chord: KeyChord, event: KeyEventLike): boolean {
  return (
    chord.code === event.code &&
    Boolean(chord.meta) === event.metaKey &&
    Boolean(chord.ctrl) === event.ctrlKey &&
    Boolean(chord.shift) === event.shiftKey &&
    Boolean(chord.alt) === event.altKey
  );
}

/**
 * The command `event` triggers, if any. `inTerminal` says whether the focus
 * is in a terminal, where only `global` bindings apply. Nothing is matched
 * during a composition (input methods, dead keys).
 */
export function matchBinding(
  bindings: readonly Binding[],
  event: KeyEventLike,
  inTerminal: boolean,
): CommandId | null {
  if (event.isComposing || event.keyCode === 229) return null;
  for (const binding of bindings) {
    if (inTerminal && binding.scope !== "global") continue;
    if (chordMatches(binding.chord, event)) return binding.command;
  }
  return null;
}

const KEY_NAMES: Record<string, string> = {
  BracketRight: "]",
  BracketLeft: "[",
  ArrowLeft: "←",
  ArrowRight: "→",
  ArrowUp: "↑",
  ArrowDown: "↓",
};

function keyName(code: string): string {
  if (code in KEY_NAMES) return KEY_NAMES[code] ?? code;
  if (code.startsWith("Key")) return code.slice(3);
  if (code.startsWith("Digit")) return code.slice(5);
  return code;
}

/** How a chord is written in menus and tooltips: `⌘⇧T` or `Ctrl+Shift+T`. */
export function formatChord(chord: KeyChord, platform: Platform): string {
  const key = keyName(chord.code);
  if (platform === "macos") {
    return `${chord.ctrl ? "⌃" : ""}${chord.alt ? "⌥" : ""}${chord.shift ? "⇧" : ""}${
      chord.meta ? "⌘" : ""
    }${key}`;
  }
  const parts: string[] = [];
  if (chord.ctrl) parts.push("Ctrl");
  if (chord.alt) parts.push("Alt");
  if (chord.shift) parts.push("Shift");
  if (chord.meta) parts.push("Win");
  parts.push(key);
  return parts.join("+");
}

/** The shortcut of `command`, written for the user, or `null` without one. */
export function shortcutOf(
  bindings: readonly Binding[],
  command: CommandId,
  platform: Platform,
): string | null {
  const binding = bindings.find((candidate) => candidate.command === command);
  return binding ? formatChord(binding.chord, platform) : null;
}

/**
 * Keys that must always reach the program in the terminal, on every
 * platform. No `global` binding may match one of them.
 *
 * - Control keys of shells and editors: Ctrl+A to Ctrl+Z, Ctrl+digits
 *   (several are legacy control codes), Ctrl+[ ] \ / _ and Ctrl+Space.
 * - Claude Code: Escape, Shift+Tab (mode), Ctrl+R (history), Ctrl+O
 *   (transcript), Ctrl+T (tasks), Ctrl+B (background), Ctrl+G (editor),
 *   Ctrl+V (paste an image), Ctrl+_ (undo), Ctrl+J, Alt+P and Alt+T, Alt+V,
 *   Shift+Enter and Alt+Enter (new line).
 * - Meta (Alt or Option) with a letter, digit or arrow: readline and emacs.
 * - Plain navigation and function keys, and Ctrl or Shift with the arrows.
 */
export const PTY_RESERVED: readonly KeyChord[] = (() => {
  const keys: KeyChord[] = [];
  const letters = "ABCDEFGHIJKLMNOPQRSTUVWXYZ".split("").map((letter) => `Key${letter}`);
  const digits = "0123456789".split("").map((digit) => `Digit${digit}`);
  const punctuation = ["BracketLeft", "BracketRight", "Backslash", "Slash", "Minus", "Space"];
  for (const code of [...letters, ...digits, ...punctuation]) {
    keys.push({ code, ctrl: true });
    keys.push({ code, alt: true });
    keys.push({ code });
    keys.push({ code, shift: true });
  }
  // Ctrl+_ is typed with Shift on most layouts.
  keys.push({ code: "Minus", ctrl: true, shift: true });
  const navigation = [
    "Escape",
    "Tab",
    "Enter",
    "Backspace",
    "Delete",
    "Insert",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "ArrowLeft",
    "ArrowRight",
    "ArrowUp",
    "ArrowDown",
    ...Array.from({ length: 12 }, (_, index) => `F${String(index + 1)}`),
  ];
  for (const code of navigation) {
    keys.push({ code });
    keys.push({ code, shift: true });
    keys.push({ code, ctrl: true });
    keys.push({ code, alt: true });
  }
  return keys;
})();
