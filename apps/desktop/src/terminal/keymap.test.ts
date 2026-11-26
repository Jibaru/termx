import { describe, expect, it } from "vitest";

import { mapKey, type KeyStroke, SHIFT_ENTER_SEQUENCE } from "./keymap";

function stroke(key: string, modifiers: Partial<KeyStroke> = {}): KeyStroke {
  const code = /^[a-z]$/i.test(key) ? `Key${key.toUpperCase()}` : key;
  return {
    key,
    code,
    shiftKey: false,
    ctrlKey: false,
    altKey: false,
    metaKey: false,
    ...modifiers,
  };
}

const PLATFORMS = ["macos", "windows", "linux"] as const;

describe("Shift+Enter", () => {
  it.each(PLATFORMS)("sends ESC CR on %s, which Claude Code reads as a newline", (platform) => {
    expect(SHIFT_ENTER_SEQUENCE).toBe("\x1b\r");
    expect(mapKey(stroke("Enter", { shiftKey: true }), platform)).toEqual({
      kind: "send",
      data: "\x1b\r",
    });
  });

  it.each(PLATFORMS)("leaves the other Enter combinations to xterm.js on %s", (platform) => {
    expect(mapKey(stroke("Enter"), platform)).toEqual({ kind: "default" });
    for (const modifiers of [
      { altKey: true },
      { ctrlKey: true },
      { metaKey: true },
      { shiftKey: true, ctrlKey: true },
      { shiftKey: true, altKey: true },
    ]) {
      expect(mapKey(stroke("Enter", modifiers), platform)).toEqual({ kind: "default" });
    }
  });
});

describe("copy and paste", () => {
  it.each(["windows", "linux"] as const)("use Ctrl+Shift+C and Ctrl+Shift+V on %s", (platform) => {
    const chord = { ctrlKey: true, shiftKey: true };

    // With Shift held, `key` is the capital letter.
    expect(mapKey(stroke("C", chord), platform)).toEqual({ kind: "copy" });
    expect(mapKey(stroke("V", chord), platform)).toEqual({ kind: "paste" });
    // The physical key decides, so Caps Lock changes nothing.
    expect(mapKey({ ...stroke("c", chord), code: "KeyC" }, platform)).toEqual({ kind: "copy" });
  });

  it.each(["windows", "linux"] as const)(
    "also use Ctrl+Insert and Shift+Insert on %s",
    (platform) => {
      expect(mapKey(stroke("Insert", { ctrlKey: true }), platform)).toEqual({ kind: "copy" });
      expect(mapKey(stroke("Insert", { shiftKey: true }), platform)).toEqual({ kind: "paste" });
      expect(mapKey(stroke("Insert"), platform)).toEqual({ kind: "default" });
    },
  );

  it.each(PLATFORMS)("leave Ctrl+C and Ctrl+V to the program on %s", (platform) => {
    expect(mapKey(stroke("c", { ctrlKey: true }), platform)).toEqual({ kind: "default" });
    expect(mapKey(stroke("v", { ctrlKey: true }), platform)).toEqual({ kind: "default" });
  });

  it("are left to the webview on macOS, where Cmd+C and Cmd+V are not keys", () => {
    expect(mapKey(stroke("c", { metaKey: true }), "macos")).toEqual({ kind: "default" });
    expect(mapKey(stroke("v", { metaKey: true }), "macos")).toEqual({ kind: "default" });
    // The Windows and Linux chords mean nothing special there.
    const chord = { ctrlKey: true, shiftKey: true };
    expect(mapKey(stroke("C", chord), "macos")).toEqual({ kind: "default" });
    expect(mapKey(stroke("V", chord), "macos")).toEqual({ kind: "default" });
    expect(mapKey(stroke("Insert", { shiftKey: true }), "macos")).toEqual({ kind: "default" });
  });
});

describe("the keys programs depend on", () => {
  it.each(PLATFORMS)("are all left to xterm.js on %s", (platform) => {
    for (const key of [
      stroke("c", { ctrlKey: true }),
      stroke("d", { ctrlKey: true }),
      stroke("z", { ctrlKey: true }),
      stroke("Escape"),
      stroke("Tab", { shiftKey: true }),
      stroke("ArrowUp"),
      stroke("ArrowLeft", { altKey: true }),
      stroke("ArrowRight", { ctrlKey: true }),
      stroke("ArrowUp", { shiftKey: true }),
      stroke("b", { altKey: true }),
      stroke("Backspace", { altKey: true }),
    ]) {
      expect(mapKey(key, platform)).toEqual({ kind: "default" });
    }
  });
});
