import type { KeyEvent } from "./page.ts";

/**
 * Descriptions of key strokes, as a webview reports them for a US keyboard.
 *
 * The suite cannot press physical keys: it delivers `KeyboardEvent`s to the
 * terminal (see `pageLibrary().keys`). These helpers fill in what a real
 * event carries: `key`, `code`, the legacy `keyCode` and, for modifiers,
 * which of the two keys it is.
 */

/** Modifiers held while a key is pressed. */
export interface Modifiers {
  shift?: boolean;
  ctrl?: boolean;
  alt?: boolean;
  meta?: boolean;
}

/** `code` and `keyCode` of the keys that type punctuation, with and without Shift. */
const PUNCTUATION: Record<string, [code: string, keyCode: number, shifted: boolean]> = {
  " ": ["Space", 32, false],
  ";": ["Semicolon", 186, false],
  ":": ["Semicolon", 186, true],
  "=": ["Equal", 187, false],
  "+": ["Equal", 187, true],
  ",": ["Comma", 188, false],
  "<": ["Comma", 188, true],
  "-": ["Minus", 189, false],
  _: ["Minus", 189, true],
  ".": ["Period", 190, false],
  ">": ["Period", 190, true],
  "/": ["Slash", 191, false],
  "?": ["Slash", 191, true],
  "`": ["Backquote", 192, false],
  "~": ["Backquote", 192, true],
  "[": ["BracketLeft", 219, false],
  "{": ["BracketLeft", 219, true],
  "\\": ["Backslash", 220, false],
  "|": ["Backslash", 220, true],
  "]": ["BracketRight", 221, false],
  "}": ["BracketRight", 221, true],
  "'": ["Quote", 222, false],
  '"': ["Quote", 222, true],
  ")": ["Digit0", 48, true],
  "!": ["Digit1", 49, true],
  "@": ["Digit2", 50, true],
  "#": ["Digit3", 51, true],
  $: ["Digit4", 52, true],
  "%": ["Digit5", 53, true],
  "^": ["Digit6", 54, true],
  "&": ["Digit7", 55, true],
  "*": ["Digit8", 56, true],
  "(": ["Digit9", 57, true],
};

/** The key that types `character`, and whether it needs Shift. */
function keyOf(character: string): { code: string; keyCode: number; shifted: boolean } {
  if (/^[a-z]$/i.test(character)) {
    const upper = character.toUpperCase();
    return {
      code: `Key${upper}`,
      keyCode: upper.charCodeAt(0),
      shifted: character === upper,
    };
  }
  if (/^[0-9]$/.test(character)) {
    return { code: `Digit${character}`, keyCode: character.charCodeAt(0), shifted: false };
  }
  const punctuation = PUNCTUATION[character];
  if (!punctuation) {
    throw new Error(`no key of a US keyboard types ${JSON.stringify(character)}`);
  }
  const [code, keyCode, shifted] = punctuation;
  return { code, keyCode, shifted };
}

const SHIFT = { key: "Shift", code: "ShiftLeft", keyCode: 16, location: 1 } as const;

/** The key strokes that type `text`, which must be printable ASCII. */
export function typing(text: string): KeyEvent[] {
  const events: KeyEvent[] = [];
  for (const character of text) {
    const { code, keyCode, shifted } = keyOf(character);
    if (shifted) events.push({ type: "down", ...SHIFT, shift: true });
    events.push({
      type: "press",
      key: character,
      code,
      keyCode,
      shift: shifted,
      text: character,
    });
    if (shifted) events.push({ type: "up", ...SHIFT });
  }
  return events;
}

/** Which of the two keys of a modifier is held. */
export type Side = "left" | "right";

function modifierKey(
  name: "Shift" | "Control" | "Alt" | "Meta",
  side: Side,
): Pick<KeyEvent, "key" | "code" | "keyCode" | "location"> {
  const keyCodes = { Shift: 16, Control: 17, Alt: 18, Meta: 91 };
  return {
    key: name,
    code: `${name}${side === "left" ? "Left" : "Right"}`,
    keyCode: keyCodes[name],
    location: side === "left" ? 1 : 2,
  };
}

/**
 * The events of pressing `stroke` with modifiers held: each modifier goes
 * down first and comes up last, as when a person types the chord.
 */
export function chord(
  stroke: Omit<KeyEvent, "type" | keyof Modifiers>,
  modifiers: Modifiers & { side?: Side },
): KeyEvent[] {
  const side = modifiers.side ?? "left";
  const held: Required<Modifiers> = { shift: false, ctrl: false, alt: false, meta: false };
  const names = [
    ["ctrl", "Control"],
    ["alt", "Alt"],
    ["shift", "Shift"],
    ["meta", "Meta"],
  ] as const;

  const events: KeyEvent[] = [];
  for (const [flag, name] of names) {
    if (!modifiers[flag]) continue;
    held[flag] = true;
    events.push({ type: "down", ...modifierKey(name, side), ...held });
  }
  events.push({ type: "press", ...stroke, ...held });
  for (const [flag, name] of [...names].reverse()) {
    if (!modifiers[flag]) continue;
    held[flag] = false;
    events.push({ type: "up", ...modifierKey(name, side), ...held });
  }
  return events;
}

/** The Enter key. It types a carriage return unless a handler takes it. */
export const ENTER = { key: "Enter", code: "Enter", keyCode: 13, text: "\r" } as const;

/** Enter, alone or with modifiers. */
export function enter(modifiers: Modifiers & { side?: Side } = {}): KeyEvent[] {
  return chord(ENTER, modifiers);
}

/** Ctrl and a letter, such as Ctrl+C. A webview sends no `keypress` for it. */
export function control(letter: string): KeyEvent[] {
  const { code, keyCode } = keyOf(letter);
  return chord({ key: letter, code, keyCode }, { ctrl: true });
}

/**
 * Option and a letter on a Mac keyboard. macOS reports the character the
 * layout assigns to the combination (`produces`, such as "å" for Option+A)
 * as the `key`, and types it unless the application takes the key.
 */
export function option(letter: string, produces: string, side: Side): KeyEvent[] {
  const { code, keyCode } = keyOf(letter);
  return chord({ key: produces, code, keyCode, text: produces }, { alt: true, side });
}
