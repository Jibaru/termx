import { describe, expect, it } from "vitest";

import type { Platform } from "../lib/platform";
import {
  type Binding,
  chordMatches,
  COMMAND_LABELS,
  defaultBindings,
  formatChord,
  type KeyChord,
  matchBinding,
  MENU_ENTRIES,
  menuShortcuts,
  PTY_RESERVED,
  resolveBindings,
  shortcutOf,
} from "./keybindings";

const PLATFORMS: Platform[] = ["macos", "windows", "linux"];

function event(chord: KeyChord) {
  return {
    code: chord.code,
    metaKey: chord.meta ?? false,
    ctrlKey: chord.ctrl ?? false,
    shiftKey: chord.shift ?? false,
    altKey: chord.alt ?? false,
    isComposing: false,
  };
}

function chordOf(bindings: Binding[], command: Binding["command"]): KeyChord | undefined {
  return bindings.find((binding) => binding.command === command)?.chord;
}

describe("default key bindings", () => {
  it("use Cmd on macOS", () => {
    const bindings = defaultBindings("macos");
    expect(chordOf(bindings, "newSession")).toEqual({ code: "KeyT", meta: true, shift: true });
    expect(chordOf(bindings, "newSiblingSession")).toEqual({
      code: "KeyN",
      meta: true,
      shift: true,
    });
    expect(chordOf(bindings, "closeSession")).toEqual({ code: "KeyW", meta: true, shift: true });
    expect(chordOf(bindings, "splitRight")).toEqual({ code: "KeyD", meta: true });
    expect(chordOf(bindings, "splitDown")).toEqual({ code: "KeyD", meta: true, shift: true });
    expect(chordOf(bindings, "focusPaneLeft")).toEqual({
      code: "ArrowLeft",
      meta: true,
      alt: true,
    });
    expect(chordOf(bindings, "nextSession")).toEqual({
      code: "BracketRight",
      meta: true,
      shift: true,
    });
    expect(chordOf(bindings, "jumpToSession3")).toEqual({ code: "Digit3", meta: true });
    expect(chordOf(bindings, "toggleSidebar")).toEqual({ code: "KeyB", meta: true });
    // Cmd+W closes a pane, never the window and its sessions.
    expect(chordOf(bindings, "closePane")).toEqual({ code: "KeyW", meta: true });
  });

  it.each(["windows", "linux"] as const)("use Ctrl+Shift on %s", (platform) => {
    const bindings = defaultBindings(platform);
    expect(chordOf(bindings, "newSession")).toEqual({ code: "KeyT", ctrl: true, shift: true });
    expect(chordOf(bindings, "splitRight")).toEqual({ code: "KeyD", ctrl: true, shift: true });
    expect(chordOf(bindings, "splitDown")).toEqual({ code: "KeyE", ctrl: true, shift: true });
    expect(chordOf(bindings, "toggleSidebar")).toEqual({
      code: "KeyB",
      ctrl: true,
      shift: true,
    });
    expect(chordOf(bindings, "jumpToSession1")).toEqual({
      code: "Digit1",
      ctrl: true,
      shift: true,
    });
    // Not Ctrl+Alt+arrows, which GNOME and Intel graphics drivers take.
    expect(chordOf(bindings, "focusPaneUp")).toEqual({ code: "ArrowUp", ctrl: true, shift: true });
    for (const binding of bindings) {
      expect(Boolean(binding.chord.ctrl && binding.chord.alt && !binding.chord.shift)).toBe(false);
    }
  });

  it.each(PLATFORMS)(
    "give every command at most one chord, and no chord twice (%s)",
    (platform) => {
      const bindings = defaultBindings(platform);
      const commands = bindings.map((binding) => binding.command);
      expect(new Set(commands).size).toBe(commands.length);
      const chords = bindings.map((binding) => JSON.stringify(event(binding.chord)));
      expect(new Set(chords).size).toBe(chords.length);
    },
  );

  it.each(PLATFORMS)("never take a key the program in the terminal needs (%s)", (platform) => {
    const bindings = defaultBindings(platform);
    for (const reserved of PTY_RESERVED) {
      expect(
        matchBinding(bindings, event(reserved), true),
        `${JSON.stringify(reserved)} is taken`,
      ).toBeNull();
    }
  });

  it.each(PLATFORMS)("leave copy and paste to the terminal (%s)", (platform) => {
    for (const binding of defaultBindings(platform)) {
      expect(["KeyC", "KeyV"]).not.toContain(binding.chord.code);
    }
  });

  it("covers the keys of the shell and of Claude Code in the reserved list", () => {
    const reserved = (chord: KeyChord) =>
      PTY_RESERVED.some((candidate) => chordMatches(candidate, event(chord)));
    for (const letter of ["C", "D", "Z", "R", "L", "B", "O", "T", "G", "V", "J"]) {
      expect(reserved({ code: `Key${letter}`, ctrl: true })).toBe(true);
    }
    expect(reserved({ code: "Tab", shift: true })).toBe(true);
    expect(reserved({ code: "Escape" })).toBe(true);
    expect(reserved({ code: "Enter", shift: true })).toBe(true);
    expect(reserved({ code: "Enter", alt: true })).toBe(true);
    expect(reserved({ code: "KeyP", alt: true })).toBe(true);
    expect(reserved({ code: "F2" })).toBe(true);
  });
});

describe("matching", () => {
  const bindings = defaultBindings("linux");

  it("finds the command of a chord", () => {
    expect(matchBinding(bindings, event({ code: "KeyT", ctrl: true, shift: true }), true)).toBe(
      "newSession",
    );
  });

  it("needs the exact modifiers", () => {
    expect(
      matchBinding(bindings, event({ code: "KeyT", ctrl: true, shift: true, alt: true }), true),
    ).toBeNull();
  });

  it("applies F2 outside the terminal only", () => {
    expect(matchBinding(bindings, event({ code: "F2" }), false)).toBe("renameSession");
    expect(matchBinding(bindings, event({ code: "F2" }), true)).toBeNull();
  });

  it("matches nothing during a composition", () => {
    expect(
      matchBinding(
        bindings,
        { ...event({ code: "KeyT", ctrl: true, shift: true }), isComposing: true },
        false,
      ),
    ).toBeNull();
  });
});

describe("the menu bar of macOS", () => {
  it("lists commands that exist, once", () => {
    const commands = MENU_ENTRIES.map((entry) => entry.command);
    expect(new Set(commands).size).toBe(commands.length);
    for (const command of commands) expect(Object.keys(COMMAND_LABELS)).toContain(command);
  });

  it("takes its shortcuts from the bindings, overrides included", () => {
    const shortcuts = new Map(
      menuShortcuts(defaultBindings("macos")).map((each) => [each.command, each.accelerator]),
    );
    expect(shortcuts.get("closePane")).toBe("Cmd+KeyW");
    expect(shortcuts.get("newSession")).toBe("Shift+Cmd+KeyT");
    expect(shortcuts.get("nextSession")).toBe("Shift+Cmd+BracketRight");
    // F2 belongs to the program in a terminal: the menu does not take it.
    expect(shortcuts.get("renameSession")).toBeNull();

    const overridden = menuShortcuts(
      resolveBindings("macos", { splitRight: { code: "KeyR", meta: true }, toggleSidebar: null }),
    );
    expect(overridden.find((each) => each.command === "splitRight")?.accelerator).toBe("Cmd+KeyR");
    expect(overridden.find((each) => each.command === "toggleSidebar")?.accelerator).toBeNull();
  });
});

describe("overrides", () => {
  it("replace a chord, or remove a binding", () => {
    const bindings = resolveBindings("macos", {
      splitRight: { code: "KeyR", meta: true },
      toggleSidebar: null,
    });
    expect(chordOf(bindings, "splitRight")).toEqual({ code: "KeyR", meta: true });
    expect(chordOf(bindings, "toggleSidebar")).toBeUndefined();
    expect(chordOf(bindings, "newSession")).toEqual(
      chordOf(defaultBindings("macos"), "newSession"),
    );
  });
});

describe("how shortcuts are written", () => {
  it("uses the symbols of macOS", () => {
    expect(formatChord({ code: "KeyT", meta: true, shift: true }, "macos")).toBe("⇧⌘T");
    expect(formatChord({ code: "ArrowLeft", meta: true, alt: true }, "macos")).toBe("⌥⌘←");
  });

  it("uses words elsewhere", () => {
    expect(formatChord({ code: "BracketRight", ctrl: true, shift: true }, "windows")).toBe(
      "Ctrl+Shift+]",
    );
    expect(shortcutOf(defaultBindings("linux"), "jumpToSession2", "linux")).toBe("Ctrl+Shift+2");
    expect(shortcutOf([], "newSession", "linux")).toBeNull();
  });
});
