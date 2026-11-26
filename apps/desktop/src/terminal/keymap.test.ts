import { describe, expect, it } from "vitest";

import { type KeyStroke, mapKey, OptionKeys, sanitizePaste, SHIFT_ENTER_SEQUENCE } from "./keymap";

function stroke(key: string, modifiers: Partial<KeyStroke> = {}): KeyStroke {
  const code = /^[a-z]$/i.test(key) ? `Key${key.toUpperCase()}` : key;
  return {
    key,
    code,
    shiftKey: false,
    ctrlKey: false,
    altKey: false,
    metaKey: false,
    isComposing: false,
    keyCode: 0,
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
      { ctrlKey: true },
      { metaKey: true },
      { shiftKey: true, ctrlKey: true },
      { altKey: true, metaKey: true },
    ]) {
      expect(mapKey(stroke("Enter", modifiers), platform)).toEqual({ kind: "default" });
    }
  });
});

describe("Option+Enter", () => {
  it.each(PLATFORMS)("sends the same bytes as Shift+Enter on %s", (platform) => {
    const expected = { kind: "send", data: SHIFT_ENTER_SEQUENCE };

    expect(mapKey(stroke("Enter", { altKey: true }), platform)).toEqual(expected);
    expect(mapKey(stroke("Enter", { altKey: true, shiftKey: true }), platform)).toEqual(expected);
  });
});

describe("during a composition", () => {
  it.each(PLATFORMS)("no key is intercepted on %s", (platform) => {
    // Enter confirms the text of an input method: it must not become a
    // newline for the program, with or without Shift.
    for (const composing of [{ isComposing: true }, { keyCode: 229 }]) {
      for (const key of [
        stroke("Enter", { shiftKey: true, ...composing }),
        stroke("Enter", { altKey: true, ...composing }),
        stroke("Process", { shiftKey: true, ...composing }),
        stroke("C", { ctrlKey: true, shiftKey: true, ...composing }),
        stroke("V", { ctrlKey: true, shiftKey: true, ...composing }),
        stroke("Insert", { shiftKey: true, ...composing }),
      ]) {
        expect(mapKey(key, platform)).toEqual({ kind: "default" });
      }
    }
  });
});

describe("the Option keys of a Mac", () => {
  const down = (code: string, altKey = true) => ({ type: "keydown", code, altKey });
  const up = (code: string, altKey = false) => ({ type: "keyup", code, altKey });

  it("only the left one is Meta in `left` mode", () => {
    const keys = new OptionKeys();
    expect(keys.actsAsMeta("left")).toBe(false);

    keys.track(down("AltLeft"));
    keys.track(down("KeyB"));
    expect(keys.actsAsMeta("left")).toBe(true);
    keys.track(up("KeyB", true));
    keys.track(up("AltLeft"));
    expect(keys.actsAsMeta("left")).toBe(false);

    keys.track(down("AltRight"));
    keys.track(down("KeyN"));
    expect(keys.actsAsMeta("left")).toBe(false);
  });

  it("the right one wins when both are held", () => {
    const keys = new OptionKeys();

    keys.track(down("AltLeft"));
    keys.track(down("AltRight"));
    expect(keys.actsAsMeta("left")).toBe(false);

    keys.track(up("AltRight", true));
    expect(keys.actsAsMeta("left")).toBe(true);
  });

  it("an Option key that was not seen going down types characters", () => {
    const keys = new OptionKeys();

    // Held before the terminal had the focus.
    keys.track(down("KeyN"));

    expect(keys.actsAsMeta("left")).toBe(false);
  });

  it("a release that was missed is noticed at the next key without Option", () => {
    const keys = new OptionKeys();
    keys.track(down("AltLeft"));

    // The window lost the focus; Option was released elsewhere.
    keys.track(down("KeyA", false));
    expect(keys.actsAsMeta("left")).toBe(false);

    keys.track(down("AltLeft"));
    keys.reset();
    expect(keys.actsAsMeta("left")).toBe(false);
  });

  it("`both` and `none` do not depend on which key is held", () => {
    const keys = new OptionKeys();
    keys.track(down("AltRight"));
    expect(keys.actsAsMeta("both")).toBe(true);
    expect(keys.actsAsMeta("none")).toBe(false);

    keys.reset();
    keys.track(down("AltLeft"));
    expect(keys.actsAsMeta("both")).toBe(true);
    expect(keys.actsAsMeta("none")).toBe(false);
  });
});

describe("sanitizePaste", () => {
  it("removes the marker that would end a bracketed paste early", () => {
    expect(sanitizePaste("safe\x1b[201~rm -rf ~\r")).toBe("saferm -rf ~\r");
    expect(sanitizePaste("\x1b[201~\x1b[201~")).toBe("");
  });

  it("removes a marker that only appears once another one is taken out", () => {
    expect(sanitizePaste("a\x1b[20\x1b[201~1~b")).toBe("ab");
  });

  it("keeps every other byte, including other escape sequences", () => {
    const text = "tab\there\r\nline two\n\x1b[200~ \x1b[31m caf\u00e9 \u2713 \x00\x7f";

    expect(sanitizePaste(text)).toBe(text);
    expect(sanitizePaste("")).toBe("");
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
