import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import { fakeBackend, pane, settle } from "../test/terminalHarness";
import { FakeResizeObserver } from "../test/xtermMock";
import { DEFAULT_TERMINAL_CONFIG, type TerminalConfig } from "./config";
import type { TerminalController as Controller } from "./TerminalController";

/**
 * The keyboard and the paste handling against the real xterm.js, as on a
 * Mac: what reaches the session for a key depends on how the key map, the
 * Option switch and xterm.js's own encoding and composition work together,
 * which stand-ins cannot show.
 *
 * xterm.js decides once, when it is loaded, which platform it runs on. It
 * is loaded here made to believe it is in a browser on macOS.
 */
let TerminalController: typeof Controller;

beforeAll(async () => {
  // Vitest runs in Node, where there is a global `process` the types of the
  // frontend know nothing about. xterm.js takes a `process` with a title for
  // Node and looks no further.
  const node = (globalThis as { process?: object }).process;
  const title = node && Object.getOwnPropertyDescriptor(node, "title");
  if (node) Reflect.deleteProperty(node, "title");
  Object.defineProperty(navigator, "platform", { configurable: true, value: "MacIntel" });
  try {
    ({ TerminalController } = await import("./TerminalController"));
  } finally {
    if (node && title) Object.defineProperty(node, "title", title);
  }
});

afterEach(() => {
  vi.unstubAllGlobals();
  document.body.replaceChildren();
});

/** A key event, with the legacy codes xterm.js still reads. */
type KeyInit = Omit<KeyboardEventInit, "keyCode" | "charCode" | "isComposing"> & {
  keyCode?: number;
  charCode?: number;
  isComposing?: boolean;
};

/** A terminal on macOS with a running session, and what was sent to it. */
async function terminalOnMac(config: Partial<TerminalConfig> = {}) {
  vi.stubGlobal("ResizeObserver", FakeResizeObserver);
  const fake = fakeBackend();
  const sent: string[] = [];
  fake.backend.writeSession = (_id, data) => {
    sent.push(data);
    return Promise.resolve();
  };
  const controller = new TerminalController({
    backend: fake.backend,
    platform: "macos",
    // happy-dom has no WebGL, and the renderer is not what is tested here.
    config: { ...DEFAULT_TERMINAL_CONFIG, renderer: "dom", ...config },
  });
  controller.attach(pane());
  await fake.creates[0]?.started();
  const textarea = controller.terminal.textarea;
  if (!textarea) throw new Error("xterm.js did not create its textarea");

  const fire = (type: "keydown" | "keypress" | "keyup", init: KeyInit): KeyboardEvent => {
    const event = new KeyboardEvent(type, { bubbles: true, cancelable: true, ...init });
    // happy-dom does not take these from the init dictionary.
    Object.defineProperty(event, "keyCode", { value: init.keyCode ?? 0 });
    Object.defineProperty(event, "charCode", { value: init.charCode ?? 0 });
    Object.defineProperty(event, "isComposing", { value: init.isComposing ?? false });
    textarea.dispatchEvent(event);
    return event;
  };

  return {
    controller,
    textarea,
    sent,
    fire,
    /** Presses and releases a key while an Option key is held. */
    withOption: (side: "left" | "right", press: () => void) => {
      const code = side === "left" ? "AltLeft" : "AltRight";
      fire("keydown", { key: "Alt", code, altKey: true, keyCode: 18 });
      press();
      fire("keyup", { key: "Alt", code, keyCode: 18 });
    },
    /** Lets xterm.js finish what it does asynchronously, then returns what was sent. */
    settled: async () => {
      await settle();
      return sent;
    },
  };
}

describe("Option as Meta on macOS, against xterm.js", () => {
  it("left Option sends ESC and the key; right Option leaves the key to the system", async () => {
    const mac = await terminalOnMac();

    mac.withOption("left", () => {
      mac.fire("keydown", { key: "∫", code: "KeyB", altKey: true, keyCode: 66 });
      mac.fire("keydown", { key: "ƒ", code: "KeyF", altKey: true, keyCode: 70 });
    });
    expect(await mac.settled()).toEqual(["\x1bb", "\x1bf"]);

    let event: KeyboardEvent | undefined;
    mac.withOption("right", () => {
      event = mac.fire("keydown", { key: "∫", code: "KeyB", altKey: true, keyCode: 66 });
    });
    // Nothing was sent for the key going down, and it was not cancelled:
    // the system goes on to type the character.
    expect(await mac.settled()).toEqual(["\x1bb", "\x1bf"]);
    expect(event?.defaultPrevented).toBe(false);
  });

  it("right Option types the characters of the layout, such as @ on a Spanish keyboard", async () => {
    const mac = await terminalOnMac();

    mac.withOption("right", () => {
      mac.fire("keydown", { key: "@", code: "Digit2", altKey: true, keyCode: 50 });
      // What the system produces for the key, which xterm.js sends as it is.
      mac.fire("keypress", { key: "@", code: "Digit2", altKey: true, keyCode: 64, charCode: 64 });
    });

    expect(await mac.settled()).toEqual(["@"]);
  });

  it("right Option starts a dead key, which is left untouched", async () => {
    const mac = await terminalOnMac();

    let dead: KeyboardEvent | undefined;
    mac.withOption("right", () => {
      // Option+N, the dead tilde of the US layout, on its way to ñ.
      dead = mac.fire("keydown", { key: "Dead", code: "KeyN", altKey: true, keyCode: 78 });
    });

    expect(await mac.settled()).toEqual([]);
    expect(dead?.defaultPrevented).toBe(false);
    expect(mac.controller.terminal.options.macOptionIsMeta).toBe(false);
  });

  it("left Option is Meta even on a key that would start a dead key", async () => {
    const mac = await terminalOnMac();

    mac.withOption("left", () => {
      mac.fire("keydown", { key: "Dead", code: "KeyE", altKey: true, keyCode: 69 });
    });

    expect(await mac.settled()).toEqual(["\x1be"]);
  });

  it("keys of a composition in progress are not sent, whichever Option key is held", async () => {
    const mac = await terminalOnMac();

    mac.textarea.dispatchEvent(new CompositionEvent("compositionstart", { data: "" }));
    mac.withOption("left", () => {
      mac.fire("keydown", {
        key: "Process",
        code: "KeyE",
        altKey: true,
        keyCode: 229,
        isComposing: true,
      });
    });
    const enter = mac.fire("keydown", {
      key: "Enter",
      code: "Enter",
      shiftKey: true,
      keyCode: 229,
      isComposing: true,
    });

    // In particular no ESC CR for the Enter that confirms the composition.
    expect(await mac.settled()).toEqual([]);
    expect(enter.defaultPrevented).toBe(false);
  });

  it("with `both`, the right Option key is Meta too", async () => {
    const mac = await terminalOnMac({ optionAsMeta: "both" });

    mac.withOption("right", () => {
      mac.fire("keydown", { key: "∫", code: "KeyB", altKey: true, keyCode: 66 });
    });

    expect(await mac.settled()).toEqual(["\x1bb"]);
  });

  it("with `none`, neither Option key is Meta", async () => {
    const mac = await terminalOnMac({ optionAsMeta: "none" });

    mac.withOption("left", () => {
      mac.fire("keydown", { key: "∫", code: "KeyB", altKey: true, keyCode: 66 });
    });

    expect(await mac.settled()).toEqual([]);
  });

  it.each(["left", "both", "none"] as const)(
    "Option+Enter and Shift+Enter send ESC CR exactly once when set to `%s`",
    async (optionAsMeta) => {
      const mac = await terminalOnMac({ optionAsMeta });

      mac.withOption("left", () => {
        mac.fire("keydown", { key: "Enter", code: "Enter", altKey: true, keyCode: 13 });
      });
      mac.withOption("right", () => {
        mac.fire("keydown", { key: "Enter", code: "Enter", altKey: true, keyCode: 13 });
      });
      mac.fire("keydown", { key: "Enter", code: "Enter", shiftKey: true, keyCode: 13 });

      // One write at a time, so what was typed meanwhile is sent together.
      expect((await mac.settled()).join("")).toBe("\x1b\r\x1b\r\x1b\r");
    },
  );
});

describe("the keys programs depend on, against xterm.js", () => {
  it("are encoded the way xterm does", async () => {
    const mac = await terminalOnMac();

    mac.fire("keydown", { key: "c", code: "KeyC", ctrlKey: true, keyCode: 67 });
    mac.fire("keydown", { key: "d", code: "KeyD", ctrlKey: true, keyCode: 68 });
    mac.fire("keydown", { key: "z", code: "KeyZ", ctrlKey: true, keyCode: 90 });
    mac.fire("keydown", { key: "Escape", code: "Escape", keyCode: 27 });
    mac.fire("keydown", { key: "Enter", code: "Enter", keyCode: 13 });
    mac.fire("keydown", { key: "ArrowUp", code: "ArrowUp", keyCode: 38 });
    mac.fire("keydown", { key: "ArrowUp", code: "ArrowUp", shiftKey: true, keyCode: 38 });
    mac.fire("keydown", { key: "ArrowRight", code: "ArrowRight", ctrlKey: true, keyCode: 39 });

    expect((await mac.settled()).join("")).toBe("\x03\x04\x1a\x1b\r\x1b[A\x1b[1;2A\x1b[1;5C");
  });
});
