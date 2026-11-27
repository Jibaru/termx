import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { Platform } from "../lib/platform";
import { recordIPC } from "../test/ipcRecorder";
import {
  ACK_BATCH,
  bytes,
  fakeBackend,
  pane,
  resizePane,
  SESSION,
  settle,
} from "../test/terminalHarness";
import {
  FakeFitAddon,
  FakeResizeObserver,
  FakeTerminal,
  FakeUnicodeGraphemesAddon,
  FakeWebglAddon,
  FakeWebLinksAddon,
  resetXtermMocks,
} from "../test/xtermMock";
import { DEFAULT_TERMINAL_CONFIG, type TerminalConfig } from "./config";
import { OSC52_MAX_BYTES } from "./osc52";
import {
  MAX_WEBGL_RESTARTS,
  MIN_FIT_HEIGHT_PX,
  MIN_FIT_WIDTH_PX,
  RESIZE_DEBOUNCE_MS,
  TerminalController,
  TITLE_REPORT_INTERVAL_MS,
} from "./TerminalController";

vi.mock("@xterm/xterm", async () => {
  const mocks = await import("../test/xtermMock");
  return { Terminal: mocks.FakeTerminal };
});
vi.mock("@xterm/addon-fit", async () => {
  const mocks = await import("../test/xtermMock");
  return { FitAddon: mocks.FakeFitAddon };
});
vi.mock("@xterm/addon-webgl", async () => {
  const mocks = await import("../test/xtermMock");
  return { WebglAddon: mocks.FakeWebglAddon };
});
vi.mock("@xterm/addon-unicode-graphemes", async () => {
  const mocks = await import("../test/xtermMock");
  return { UnicodeGraphemesAddon: mocks.FakeUnicodeGraphemesAddon };
});
vi.mock("@xterm/addon-web-links", async () => {
  const mocks = await import("../test/xtermMock");
  return { WebLinksAddon: mocks.FakeWebLinksAddon };
});

interface Setup {
  platform?: Platform;
  config?: Partial<TerminalConfig>;
  /** Leave the session starting instead of answering the create call. */
  leaveStarting?: boolean;
}

/** A controller attached to a pane, with its session running. */
async function attached({ platform = "macos", config, leaveStarting = false }: Setup = {}) {
  const fake = fakeBackend();
  const controller = new TerminalController({
    sessionId: SESSION,
    backend: fake.backend,
    platform,
    config: { ...DEFAULT_TERMINAL_CONFIG, ...config },
  });
  const container = pane();
  controller.attach(container);
  const terminal = FakeTerminal.last;
  const create = fake.creates[0];
  if (!create) throw new Error("no session was created");
  if (!leaveStarting) await create.started();
  return { ...fake, controller, container, terminal, create };
}

beforeEach(() => {
  resetXtermMocks();
  vi.stubGlobal("ResizeObserver", FakeResizeObserver);
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  document.body.replaceChildren();
});

describe("starting", () => {
  it("opens xterm.js in the pane and starts the login shell at the fitted size", async () => {
    FakeFitAddon.proposed = { cols: 132, rows: 43 };

    const { controller, container, terminal, creates } = await attached();

    expect(container.contains(terminal.element ?? null)).toBe(true);
    expect(terminal.resizes).toEqual([{ cols: 132, rows: 43 }]);
    // Its session, from scratch: what it runs is the business of the session.
    expect(creates.map((call) => call.session)).toEqual([SESSION]);
    expect(creates.map((call) => call.options)).toEqual([{ mode: "fresh", cols: 132, rows: 43 }]);
    expect(controller.getStatus()).toEqual({ kind: "running", terminal: 1 });
    expect(terminal.focusCount).toBe(1);
  });

  it("does not tell the session a size it already has", async () => {
    FakeFitAddon.proposed = { cols: 132, rows: 43 };

    const { resizes } = await attached();

    expect(resizes).toEqual([]);
  });

  it("takes the look of the terminal from the configuration", async () => {
    const { terminal } = await attached();

    expect(terminal.options).toMatchObject({
      fontFamily: DEFAULT_TERMINAL_CONFIG.fontFamily,
      fontSize: 14,
      scrollback: 10_000,
      allowTransparency: false,
      theme: DEFAULT_TERMINAL_CONFIG.theme,
    });
  });

  it("activates grapheme-aware Unicode widths before anything is written", async () => {
    const { terminal, create } = await attached({ leaveStarting: true });
    create.handlers.onOutput(bytes("first output"));

    expect(terminal.unicode.activeVersion).toBe("15-graphemes");
    expect(terminal.addon(FakeUnicodeGraphemesAddon)).toBeDefined();
    expect(terminal.log.indexOf("unicode:15-graphemes")).toBeGreaterThanOrEqual(0);
    expect(terminal.log.indexOf("unicode:15-graphemes")).toBeLessThan(terminal.log.indexOf("open"));
    expect(terminal.log.indexOf("open")).toBeLessThan(terminal.log.indexOf("write"));
  });

  it("starts at the default size when the pane cannot be measured yet", () => {
    const fake = fakeBackend();
    const controller = new TerminalController({
      sessionId: SESSION,
      backend: fake.backend,
      platform: "linux",
    });
    FakeFitAddon.proposed = undefined;

    controller.attach(pane(0, 0));

    expect(fake.creates[0]?.options).toEqual({ mode: "fresh", cols: 80, rows: 24 });
    expect(FakeTerminal.last.resizes).toEqual([]);
  });

  it("resumes a session that was restored instead of starting it from scratch", () => {
    const fake = fakeBackend();
    const controller = new TerminalController({
      sessionId: "s-restored",
      autoStart: "resume",
      backend: fake.backend,
      platform: "linux",
    });

    controller.attach(pane());

    expect(fake.creates[0]?.session).toBe("s-restored");
    expect(fake.creates[0]?.options).toEqual({ mode: "resume", cols: 80, rows: 24 });
  });

  it("leaves a restored session alone until Enter starts it", async () => {
    const fake = fakeBackend();
    const controller = new TerminalController({
      sessionId: "s-restored",
      autoStart: "dormant",
      backend: fake.backend,
      platform: "linux",
    });

    controller.attach(pane());

    expect(fake.creates).toHaveLength(0);
    expect(controller.getStatus()).toEqual({ kind: "dormant" });
    expect(FakeTerminal.last.text).toContain(
      "This session was restored and is not running. Press Enter to start it.",
    );
    // Typing is not sent anywhere, and only Enter starts it.
    FakeTerminal.last.type("ls");
    expect(fake.creates).toHaveLength(0);
    // Shown again, it still waits.
    controller.detach();
    controller.attach(pane());
    expect(fake.creates).toHaveLength(0);

    FakeTerminal.last.type("\r");
    expect(fake.creates.map((call) => call.options)).toEqual([
      { mode: "resume", cols: 80, rows: 24 },
    ]);
    await fake.creates[0]?.started();
    expect(controller.getStatus()).toEqual({ kind: "running", terminal: 1 });
  });

  it("waits to be told when it must not start on its own", async () => {
    const fake = fakeBackend();
    const controller = new TerminalController({
      sessionId: SESSION,
      autoStart: false,
      backend: fake.backend,
      platform: "linux",
    });

    // Nothing can start before the terminal has a size.
    controller.start("resume");
    expect(fake.creates).toHaveLength(0);

    controller.attach(pane());
    expect(fake.creates).toHaveLength(0);
    expect(controller.getStatus()).toEqual({ kind: "idle" });
    // Nothing is typed into a session that does not run.
    FakeTerminal.last.type("x\r");
    expect(fake.creates).toHaveLength(0);

    controller.start("resume");
    expect(fake.creates[0]?.options).toEqual({ mode: "resume", cols: 80, rows: 24 });
    await fake.creates[0]?.started();
    expect(controller.getStatus()).toEqual({ kind: "running", terminal: 1 });
  });

  it("tells xterm.js which pseudo-console it talks to on Windows", async () => {
    const { terminal, create } = await attached({ platform: "windows", leaveStarting: true });

    await create.started({ windowsBuildNumber: 22631 });

    expect(terminal.options.windowsPty).toEqual({ backend: "conpty", buildNumber: 22631 });
  });

  it("shows why the shell could not be started, and tries again on Enter", async () => {
    const { controller, terminal, create, creates } = await attached({ leaveStarting: true });

    await create.failed({ command: "session_create", kind: "internal", message: "no such shell" });

    expect(controller.getStatus()).toEqual({ kind: "failed", message: "no such shell" });
    expect(terminal.text).toContain("Could not start the shell: no such shell");
    expect(terminal.text).toContain("Press Enter to try again");

    terminal.type("\r");
    expect(creates).toHaveLength(2);
    await creates[1]?.started();
    expect(controller.getStatus().kind).toBe("running");
  });
});

describe("output", () => {
  it("is written to xterm.js as the bytes that were received", async () => {
    const { terminal, create } = await attached();
    const output = Uint8Array.from([0x1b, 0x5b, 0x33, 0x31, 0x6d, 0xe2, 0x9c, 0x93]);

    create.handlers.onOutput(output);

    expect(terminal.writes).toHaveLength(1);
    expect(terminal.writes[0]?.data).toBe(output);
  });

  it("is acknowledged once xterm.js has parsed it, not when it arrives", async () => {
    const { terminal, create, acks } = await attached();

    create.handlers.onOutput(new Uint8Array(ACK_BATCH));
    expect(acks).toEqual([]);

    terminal.parseWrites();
    expect(acks).toEqual([{ id: 1, bytes: ACK_BATCH }]);
  });

  it("is acknowledged in batches of the size the session asked for", async () => {
    const { terminal, create, acks } = await attached();

    // The trickle of an interactive shell costs no message at all...
    create.handlers.onOutput(new Uint8Array(400));
    create.handlers.onOutput(new Uint8Array(400));
    terminal.parseWrites();
    expect(acks).toEqual([]);

    // ...until a batch is full, and then nothing is left out of it.
    create.handlers.onOutput(new Uint8Array(300));
    terminal.parseWrites();
    expect(acks).toEqual([{ id: 1, bytes: 1100 }]);

    create.handlers.onOutput(new Uint8Array(2 * ACK_BATCH + 1));
    terminal.parseWrites();
    expect(acks).toEqual([
      { id: 1, bytes: 1100 },
      { id: 1, bytes: 2 * ACK_BATCH + 1 },
    ]);
  });

  it("that arrives while the session is starting is acknowledged once it has started", async () => {
    const { terminal, create, acks } = await attached({ leaveStarting: true });

    create.handlers.onOutput(new Uint8Array(ACK_BATCH + 5));
    terminal.parseWrites();
    expect(terminal.writes).toHaveLength(1);
    expect(acks).toEqual([]);

    await create.started();
    expect(acks).toEqual([{ id: 1, bytes: ACK_BATCH + 5 }]);
  });
});

describe("input", () => {
  it("is sent to the session", async () => {
    const { terminal, writes } = await attached();

    terminal.type("ls\r");

    expect(writes).toMatchObject([{ id: 1, data: "ls\r", binary: false }]);
  });

  it("is sent one call at a time, in the order it was typed", async () => {
    const { terminal, writes } = await attached();

    terminal.type("a");
    terminal.type("b");
    terminal.type("c");
    // The first key is on its way; the others wait for it, together.
    expect(writes.map((write) => write.data)).toEqual(["a"]);

    await writes[0]?.done();
    expect(writes.map((write) => write.data)).toEqual(["a", "bc"]);

    await writes[1]?.done();
    terminal.type("d");
    expect(writes.map((write) => write.data)).toEqual(["a", "bc", "d"]);
  });

  it("keeps going after a write failed", async () => {
    const fake = fakeBackend();
    let attempts = 0;
    const sent: string[] = [];
    fake.backend.writeSession = (_id, data) => {
      attempts += 1;
      if (attempts === 1) return Promise.reject(new Error("write failed"));
      sent.push(data);
      return Promise.resolve();
    };
    const controller = new TerminalController({
      sessionId: SESSION,
      backend: fake.backend,
      platform: "linux",
    });
    controller.attach(pane());
    await fake.creates[0]?.started();

    FakeTerminal.last.type("x");
    FakeTerminal.last.type("y");
    await settle();

    expect(sent).toEqual(["y"]);
  });

  it("typed while the session is starting is sent once it has started", async () => {
    const { terminal, create, writes } = await attached({ leaveStarting: true });

    terminal.type("cla");
    terminal.type("ude\r");
    expect(writes).toEqual([]);

    await create.started();
    expect(writes.map((write) => write.data)).toEqual(["claude\r"]);
  });

  it("that is not text is sent as bytes, apart from the text around it", async () => {
    const { terminal, writes } = await attached();

    terminal.type("a");
    terminal.typeBinary("\x1b[M \xa0\xff");
    terminal.type("b");
    await writes[0]?.done();
    await writes[1]?.done();

    expect(writes.map(({ data, binary }) => ({ data, binary }))).toEqual([
      { data: "a", binary: false },
      { data: "\x1b[M \xa0\xff", binary: true },
      { data: "b", binary: false },
    ]);
  });
});

describe("keys", () => {
  it.each(["macos", "windows", "linux"] as const)(
    "Shift+Enter sends ESC CR on %s instead of submitting",
    async (platform) => {
      const { terminal, writes } = await attached({ platform });

      const down = terminal.key("keydown", { key: "Enter", code: "Enter", shiftKey: true });

      expect(writes.map((write) => write.data)).toEqual(["\x1b\r"]);
      // xterm.js must not send its own carriage return as well.
      expect(down.handledByXterm).toBe(false);
      expect(down.event.defaultPrevented).toBe(true);

      // The rest of the key press is swallowed without sending it again.
      const press = terminal.key("keypress", { key: "Enter", code: "Enter", shiftKey: true });
      const up = terminal.key("keyup", { key: "Enter", code: "Enter", shiftKey: true });
      expect(press.handledByXterm).toBe(false);
      expect(up.handledByXterm).toBe(false);
      expect(writes).toHaveLength(1);
    },
  );

  it("leaves Enter, Ctrl+C, Escape and the arrows to xterm.js", async () => {
    const { terminal, writes } = await attached();

    for (const init of [
      { key: "Enter", code: "Enter" },
      { key: "c", code: "KeyC", ctrlKey: true },
      { key: "d", code: "KeyD", ctrlKey: true },
      { key: "z", code: "KeyZ", ctrlKey: true },
      { key: "Escape", code: "Escape" },
      { key: "ArrowLeft", code: "ArrowLeft", altKey: true },
      { key: "ArrowUp", code: "ArrowUp", shiftKey: true },
    ]) {
      const { handledByXterm, event } = terminal.key("keydown", init);
      expect(handledByXterm).toBe(true);
      expect(event.defaultPrevented).toBe(false);
    }
    expect(writes).toEqual([]);
  });

  it.each(["windows", "linux"] as const)(
    "Ctrl+Shift+C copies the selection on %s",
    async (platform) => {
      const { terminal, clipboard, writes } = await attached({ platform });
      terminal.selection = "selected text";

      const { handledByXterm, event } = terminal.key("keydown", {
        key: "C",
        code: "KeyC",
        ctrlKey: true,
        shiftKey: true,
      });

      expect(clipboard.writes).toEqual(["selected text"]);
      expect(handledByXterm).toBe(false);
      expect(event.defaultPrevented).toBe(true);
      expect(writes).toEqual([]);
    },
  );

  it("copying without a selection leaves the clipboard alone", async () => {
    const { terminal, clipboard } = await attached({ platform: "linux" });

    terminal.key("keydown", { key: "C", code: "KeyC", ctrlKey: true, shiftKey: true });

    expect(clipboard.writes).toEqual([]);
  });

  it.each(["windows", "linux"] as const)(
    "Ctrl+Shift+V pastes through xterm.js on %s, which applies bracketed paste",
    async (platform) => {
      const { terminal, clipboard, writes } = await attached({ platform });
      clipboard.text = "line one\nline two";

      const { handledByXterm } = terminal.key("keydown", {
        key: "V",
        code: "KeyV",
        ctrlKey: true,
        shiftKey: true,
      });
      await settle();

      expect(handledByXterm).toBe(false);
      expect(terminal.pasted).toEqual(["line one\nline two"]);
      // Not sent directly: xterm.js turns the paste into input itself.
      expect(writes).toEqual([]);
    },
  );

  it("leaves Cmd+C and Cmd+V to the webview on macOS", async () => {
    const { terminal, clipboard } = await attached({ platform: "macos" });
    terminal.selection = "selected text";

    const copy = terminal.key("keydown", { key: "c", code: "KeyC", metaKey: true });
    const paste = terminal.key("keydown", { key: "v", code: "KeyV", metaKey: true });
    await settle();

    // The webview turns them into copy and paste events for xterm.js.
    expect(copy.handledByXterm).toBe(true);
    expect(paste.handledByXterm).toBe(true);
    expect(copy.event.defaultPrevented).toBe(false);
    expect(paste.event.defaultPrevented).toBe(false);
    expect(clipboard.writes).toEqual([]);
    expect(clipboard.reads).toBe(0);
  });

  it("does not treat Ctrl+Shift+C as copy on macOS", async () => {
    const { terminal, clipboard } = await attached({ platform: "macos" });
    terminal.selection = "selected text";

    const { handledByXterm } = terminal.key("keydown", {
      key: "C",
      code: "KeyC",
      ctrlKey: true,
      shiftKey: true,
    });

    expect(handledByXterm).toBe(true);
    expect(clipboard.writes).toEqual([]);
  });
});

describe("composition", () => {
  it.each(["macos", "windows", "linux"] as const)(
    "Shift+Enter that confirms an input method is not turned into a newline on %s",
    async (platform) => {
      const { terminal, writes } = await attached({ platform });

      for (const init of [
        { key: "Enter", code: "Enter", shiftKey: true, isComposing: true },
        { key: "Process", code: "Enter", shiftKey: true, keyCode: 229 },
      ]) {
        const { handledByXterm, event } = terminal.key("keydown", init);
        expect(handledByXterm).toBe(true);
        expect(event.defaultPrevented).toBe(false);
      }
      expect(writes).toEqual([]);
    },
  );
});

describe("the Option key on macOS", () => {
  const leftOption = { key: "Alt", code: "AltLeft", altKey: true, location: 1 };
  const rightOption = { key: "Alt", code: "AltRight", altKey: true, location: 2 };

  it("is Meta for the left key and types characters for the right one by default", async () => {
    const { terminal } = await attached({ platform: "macos" });
    expect(terminal.options.macOptionIsMeta).toBe(false);

    terminal.key("keydown", leftOption);
    const b = terminal.key("keydown", { key: "∫", code: "KeyB", altKey: true });
    // xterm.js does the encoding, with its switch on for this key.
    expect(terminal.options.macOptionIsMeta).toBe(true);
    expect(b.handledByXterm).toBe(true);
    terminal.key("keyup", { key: "Alt", code: "AltLeft" });

    terminal.key("keydown", rightOption);
    const n = terminal.key("keydown", { key: "Dead", code: "KeyN", altKey: true });
    expect(terminal.options.macOptionIsMeta).toBe(false);
    // Left alone: the system starts the dead key for ñ.
    expect(n.handledByXterm).toBe(true);
    expect(n.event.defaultPrevented).toBe(false);
  });

  it("does not touch a composition that is in progress", async () => {
    const { terminal, writes } = await attached({ platform: "macos" });
    terminal.key("keydown", rightOption);
    terminal.key("keydown", { key: "Dead", code: "KeyE", altKey: true });
    terminal.key("keyup", { key: "Alt", code: "AltRight" });

    // The left Option key goes down while the dead key waits for its vowel.
    terminal.key("keydown", leftOption);
    const vowel = terminal.key("keydown", {
      key: "é",
      code: "KeyE",
      altKey: true,
      isComposing: true,
      keyCode: 229,
    });

    expect(terminal.options.macOptionIsMeta).toBe(false);
    expect(vowel.handledByXterm).toBe(true);
    expect(vowel.event.defaultPrevented).toBe(false);
    expect(writes).toEqual([]);
  });

  it.each([
    ["both", true, true],
    ["none", false, false],
  ] as const)("follows the `%s` setting for both keys", async (optionAsMeta, left, right) => {
    const { terminal } = await attached({ platform: "macos", config: { optionAsMeta } });

    terminal.key("keydown", leftOption);
    terminal.key("keydown", { key: "∫", code: "KeyB", altKey: true });
    expect(terminal.options.macOptionIsMeta).toBe(left);
    terminal.key("keyup", { key: "Alt", code: "AltLeft" });

    terminal.key("keydown", rightOption);
    terminal.key("keydown", { key: "∫", code: "KeyB", altKey: true });
    expect(terminal.options.macOptionIsMeta).toBe(right);
  });

  it.each(["left", "both", "none"] as const)(
    "Option+Enter sends ESC CR with either key when set to `%s`",
    async (optionAsMeta) => {
      const { terminal, writes } = await attached({ platform: "macos", config: { optionAsMeta } });

      for (const option of [leftOption, rightOption]) {
        terminal.key("keydown", option);
        const enter = terminal.key("keydown", { key: "Enter", code: "Enter", altKey: true });
        terminal.key("keyup", { key: "Alt", code: option.code });
        expect(enter.handledByXterm).toBe(false);
      }
      await writes[0]?.done();

      expect(writes.map((write) => write.data)).toEqual(["\x1b\r", "\x1b\r"]);
    },
  );

  it("forgets a key whose release happened in another window", async () => {
    const { terminal } = await attached({ platform: "macos" });
    terminal.key("keydown", leftOption);

    window.dispatchEvent(new Event("blur"));
    // Back in the window with Option held again, by a key not seen going down.
    terminal.key("keydown", { key: "ñ", code: "KeyN", altKey: true });

    expect(terminal.options.macOptionIsMeta).toBe(false);
  });

  it("is left to xterm.js on the other platforms, where Alt is always Meta", async () => {
    const { terminal } = await attached({ platform: "linux" });
    const before = terminal.options.macOptionIsMeta;

    terminal.key("keydown", leftOption);
    terminal.key("keydown", { key: "b", code: "KeyB", altKey: true });

    expect(terminal.options.macOptionIsMeta).toBe(before);
  });
});

describe("pasting", () => {
  /** A paste performed by the webview: Cmd+V, the Edit menu, the context menu. */
  function pasteEvent(text: string): ClipboardEvent {
    const event = new Event("paste", { bubbles: true, cancelable: true }) as ClipboardEvent;
    Object.defineProperty(event, "clipboardData", {
      value: { getData: (type: string) => (type === "text/plain" ? text : "") },
    });
    return event;
  }

  it("takes the webview's own paste before xterm.js and removes the end marker", async () => {
    const { terminal } = await attached({ platform: "macos" });
    const reachedXterm = vi.fn();
    terminal.element?.addEventListener("paste", reachedXterm);

    const event = pasteEvent("echo safe\x1b[201~; rm -rf ~\r");
    terminal.element?.dispatchEvent(event);

    expect(terminal.pasted).toEqual(["echo safe; rm -rf ~\r"]);
    expect(event.defaultPrevented).toBe(true);
    expect(reachedXterm).not.toHaveBeenCalled();
  });

  it("removes the end marker from a paste made with the keyboard shortcut too", async () => {
    const { terminal, clipboard } = await attached({ platform: "linux" });
    clipboard.text = "a\x1b[20\x1b[201~1~b";

    terminal.key("keydown", { key: "V", code: "KeyV", ctrlKey: true, shiftKey: true });
    await settle();

    expect(terminal.pasted).toEqual(["ab"]);
  });

  it("pastes nothing when only the marker was on the clipboard", async () => {
    const { terminal } = await attached();

    terminal.element?.dispatchEvent(pasteEvent("\x1b[201~"));

    expect(terminal.pasted).toEqual([]);
  });

  it("stops listening when the terminal leaves the pane", async () => {
    const { controller, container, terminal } = await attached();
    controller.detach();

    container.dispatchEvent(pasteEvent("late"));

    expect(terminal.pasted).toEqual([]);
  });
});

describe("resizing", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  /** The observer of the pane the terminal is in. */
  function observer(): FakeResizeObserver {
    const [only, ...others] = FakeResizeObserver.active;
    if (!only || others.length > 0) throw new Error("expected exactly one observed pane");
    return only;
  }

  it("refits once the size of the pane has settled", async () => {
    const { terminal, resizes } = await attached();

    // A window being dragged reports many sizes in a row.
    for (const cols of [100, 110, 120]) {
      FakeFitAddon.proposed = { cols, rows: 30 };
      observer().trigger();
      vi.advanceTimersByTime(RESIZE_DEBOUNCE_MS - 1);
    }
    expect(terminal.resizes).toEqual([]);
    expect(resizes).toEqual([]);

    vi.advanceTimersByTime(1);
    expect(terminal.resizes).toEqual([{ cols: 120, rows: 30 }]);
    expect(resizes).toEqual([{ id: 1, cols: 120, rows: 30 }]);
  });

  it("sends nothing when the size in cells did not change", async () => {
    const { terminal, resizes } = await attached();

    // The pane changed by a few pixels: still 80 by 24.
    observer().trigger();
    vi.advanceTimersByTime(RESIZE_DEBOUNCE_MS);
    observer().trigger();
    vi.advanceTimersByTime(RESIZE_DEBOUNCE_MS);

    expect(terminal.resizes).toEqual([]);
    expect(resizes).toEqual([]);
  });

  it("ignores a pane that is hidden or too small to hold a terminal", async () => {
    const { terminal, container, resizes } = await attached();
    FakeFitAddon.proposed = { cols: 2, rows: 1 };

    for (const [width, height] of [
      [0, 0],
      [MIN_FIT_WIDTH_PX - 1, 600],
      [960, MIN_FIT_HEIGHT_PX - 1],
    ] as const) {
      resizePane(container, width, height);
      observer().trigger();
      vi.advanceTimersByTime(RESIZE_DEBOUNCE_MS);
    }

    expect(terminal.resizes).toEqual([]);
    expect(resizes).toEqual([]);
  });

  it("ignores measurements that are not numbers", async () => {
    const { terminal } = await attached();

    for (const proposed of [
      undefined,
      { cols: Number.NaN, rows: 24 },
      { cols: 80, rows: Infinity },
    ]) {
      FakeFitAddon.proposed = proposed;
      observer().trigger();
      vi.advanceTimersByTime(RESIZE_DEBOUNCE_MS);
    }

    expect(terminal.resizes).toEqual([]);
  });

  it("tells the session the size it missed while it was starting", async () => {
    const { create, resizes } = await attached({ leaveStarting: true });

    FakeFitAddon.proposed = { cols: 150, rows: 50 };
    observer().trigger();
    vi.advanceTimersByTime(RESIZE_DEBOUNCE_MS);
    expect(resizes).toEqual([]);

    await create.started();
    expect(resizes).toEqual([{ id: 1, cols: 150, rows: 50 }]);
  });
});

describe("when the process exits", () => {
  it("says so in the terminal, with the exit code and how to restart", async () => {
    const { controller, terminal, create } = await attached();
    create.handlers.onOutput(bytes("logout\r\n"));

    create.handlers.onExit({ code: 3, signal: null });

    expect(controller.getStatus()).toEqual({ kind: "exited", exit: { code: 3, signal: null } });
    const text = terminal.text;
    expect(text).toContain("The process exited with code 3.");
    expect(text).toContain("Press Enter to restart.");
    // After the last output, and outside the alternate screen a program
    // may have died in.
    expect(text.indexOf("logout")).toBeLessThan(text.indexOf("The process exited"));
    expect(text.indexOf("\x1b[?1047l")).toBeLessThan(text.indexOf("The process exited"));
    // Not with mode 1049, which would also move the cursor to where it was
    // last saved, and the notice with it.
    expect(text).not.toContain("\x1b[?1049l");
  });

  it("names the signal when one ended the process", async () => {
    const { terminal, create } = await attached();

    create.handlers.onExit({ code: null, signal: "Hangup" });

    expect(terminal.text).toContain("The process was terminated (Hangup).");
  });

  it("sends nothing more to the session that is gone", async () => {
    const { terminal, create, writes, resizes, acks } = await attached();
    create.handlers.onExit({ code: 0, signal: null });

    terminal.type("ls");
    terminal.resize(100, 30);
    terminal.parseWrites();

    expect(writes).toEqual([]);
    expect(resizes).toEqual([]);
    expect(acks).toEqual([]);
  });

  it("starts a new session in the same terminal on Enter", async () => {
    const { controller, terminal, create, creates, writes } = await attached();
    create.handlers.onExit({ code: 0, signal: null });
    terminal.resize(100, 30);

    terminal.type("x");
    expect(creates).toHaveLength(1);

    terminal.type("\r");
    expect(creates).toHaveLength(2);
    expect(creates[1]?.options).toEqual({ mode: "fresh", cols: 100, rows: 30 });
    expect(controller.getStatus()).toEqual({ kind: "starting" });
    // The Enter that restarted is not typed into the new shell.
    await creates[1]?.started();
    expect(writes).toEqual([]);
    expect(controller.getStatus()).toEqual({ kind: "running", terminal: 2 });
    expect(FakeTerminal.instances).toHaveLength(1);

    terminal.type("echo again\r");
    expect(writes).toMatchObject([{ id: 2, data: "echo again\r" }]);
  });

  it("ignores what the previous session still delivers after a restart", async () => {
    const { terminal, create, creates, acks } = await attached();
    create.handlers.onExit({ code: 0, signal: null });
    terminal.type("\r");
    await creates[1]?.started();
    const written = terminal.writes.length;

    create.handlers.onOutput(bytes("late"));
    create.handlers.onExit({ code: 1, signal: null });

    expect(terminal.writes).toHaveLength(written);
    expect(acks).toEqual([]);
  });

  it("handles a process that exits before its session has been announced", async () => {
    const { controller, terminal, create, writes } = await attached({ leaveStarting: true });

    create.handlers.onOutput(bytes("command not found\r\n"));
    create.handlers.onExit({ code: 127, signal: null });
    await create.started();

    expect(controller.getStatus().kind).toBe("exited");
    expect(terminal.text).toContain("The process exited with code 127.");
    terminal.type("x");
    expect(writes).toEqual([]);
  });

  it("tells whoever watches the status", async () => {
    const { controller, create } = await attached();
    const seen: string[] = [];
    const stop = controller.subscribe(() => seen.push(controller.getStatus().kind));

    create.handlers.onExit({ code: 0, signal: null });
    stop();
    FakeTerminal.last.type("\r");

    expect(seen).toEqual(["exited"]);
  });
});

describe("the renderer", () => {
  it("is WebGL when it is available", async () => {
    const { controller, terminal } = await attached();

    expect(controller.renderer).toBe("webgl");
    expect(terminal.addon(FakeWebglAddon)).toBeDefined();
    // Loaded once the terminal is open: it needs the canvas.
    expect(FakeWebglAddon.instances).toHaveLength(1);
  });

  it("falls back to the DOM renderer when WebGL is not available", async () => {
    FakeWebglAddon.unavailable = true;
    const ipc = recordIPC();

    const { controller, create } = await attached();

    expect(controller.renderer).toBe("dom");
    // The terminal works all the same.
    create.handlers.onOutput(bytes("still works"));
    expect(FakeTerminal.last.text).toContain("still works");
    await vi.waitFor(() => {
      expect(ipc.logged()).toHaveLength(1);
    });
    expect(ipc.logged()[0]).toMatchObject({ level: "warn", source: "terminal-renderer" });
  });

  it("falls back to the DOM renderer when the WebGL context is lost", async () => {
    const ipc = recordIPC();
    const { controller } = await attached();
    const webgl = FakeWebglAddon.instances[0];
    if (!webgl) throw new Error("the WebGL addon was not loaded");

    webgl.loseContext();

    expect(webgl.disposed).toBe(true);
    expect(controller.renderer).toBe("dom");
    await vi.waitFor(() => {
      expect(ipc.logged()).toHaveLength(1);
    });
    expect(ipc.logged()[0]?.message).toContain("WebGL context was lost");
  });

  it("stays on the DOM renderer after a lost context when set to `auto`", async () => {
    recordIPC();
    const { controller } = await attached({ config: { renderer: "auto" } });

    FakeWebglAddon.instances[0]?.loseContext();

    expect(controller.renderer).toBe("dom");
    expect(FakeWebglAddon.instances).toHaveLength(1);
  });

  it("starts WebGL again after a lost context when set to `webgl`, a few times", async () => {
    const ipc = recordIPC();
    const { controller } = await attached({ config: { renderer: "webgl" } });

    for (let loss = 1; loss <= MAX_WEBGL_RESTARTS; loss += 1) {
      FakeWebglAddon.instances.at(-1)?.loseContext();
      expect(controller.renderer).toBe("webgl");
      expect(FakeWebglAddon.instances).toHaveLength(loss + 1);
    }

    // A machine that keeps losing the context ends up on the DOM renderer.
    FakeWebglAddon.instances.at(-1)?.loseContext();
    expect(controller.renderer).toBe("dom");
    expect(FakeWebglAddon.instances).toHaveLength(MAX_WEBGL_RESTARTS + 1);
    await vi.waitFor(() => {
      expect(ipc.logged().at(-1)?.message).toContain("now uses the DOM renderer");
    });
  });

  it("can be configured to never use WebGL", () => {
    const fake = fakeBackend();
    const controller = new TerminalController({
      sessionId: SESSION,
      backend: fake.backend,
      platform: "linux",
      config: { ...DEFAULT_TERMINAL_CONFIG, renderer: "dom" },
    });

    controller.attach(pane());

    expect(controller.renderer).toBe("dom");
    expect(FakeWebglAddon.instances).toEqual([]);
  });
});

describe("the clipboard requests of programs (OSC 52)", () => {
  it("lets a program write the clipboard", async () => {
    const { terminal, clipboard } = await attached();
    const handler = terminal.oscHandlers.get(52);

    expect(handler?.(`c;${btoa("copied by a program")}`)).toBe(true);

    expect(clipboard.writes).toEqual(["copied by a program"]);
  });

  it("ignores a program writing the clipboard when the setting is off", async () => {
    const { terminal, clipboard } = await attached({ config: { allowOsc52Write: false } });

    // Still reported as handled: the sequence must not show up as text.
    expect(terminal.oscHandlers.get(52)?.(`c;${btoa("unwanted")}`)).toBe(true);

    expect(clipboard.writes).toEqual([]);
  });

  it("ignores a payload larger than the limit", async () => {
    const { terminal, clipboard } = await attached();

    const handled = terminal.oscHandlers.get(52)?.(`c;${btoa("x".repeat(OSC52_MAX_BYTES + 3))}`);

    expect(handled).toBe(true);
    expect(clipboard.writes).toEqual([]);
  });

  it("never lets a program read the clipboard", async () => {
    const { terminal, clipboard, writes } = await attached();
    clipboard.text = "a secret";

    // Reported as handled, so that nothing else answers it either.
    expect(terminal.oscHandlers.get(52)?.("c;?")).toBe(true);
    await settle();

    expect(clipboard.reads).toBe(0);
    expect(clipboard.writes).toEqual([]);
    expect(writes).toEqual([]);
  });
});

describe("links", () => {
  function click(modifiers: MouseEventInit = {}): MouseEvent {
    return new MouseEvent("click", modifiers);
  }

  it("open in the browser on Cmd+click on macOS and Ctrl+click elsewhere", async () => {
    const mac = await attached({ platform: "macos" });
    const macLinks = mac.terminal.addon(FakeWebLinksAddon);
    macLinks?.handler(click({ metaKey: true }), "https://example.com/a");
    macLinks?.handler(click({ ctrlKey: true }), "https://example.com/not-on-mac");
    expect(mac.opened).toEqual(["https://example.com/a"]);

    const linux = await attached({ platform: "linux" });
    const linuxLinks = linux.terminal.addon(FakeWebLinksAddon);
    linuxLinks?.handler(click({ ctrlKey: true }), "http://example.com/b");
    linuxLinks?.handler(click({ metaKey: true }), "https://example.com/not-on-linux");
    expect(linux.opened).toEqual(["http://example.com/b"]);
  });

  it("do not open on a plain click, which belongs to the program", async () => {
    const { terminal, opened } = await attached();

    terminal.addon(FakeWebLinksAddon)?.handler(click(), "https://example.com");

    expect(opened).toEqual([]);
  });

  it("open for OSC 8 hyperlinks too, but only to http and https", async () => {
    const { terminal, opened } = await attached({ platform: "linux" });
    const handler = terminal.options.linkHandler as {
      activate: (event: MouseEvent, uri: string) => void;
    };

    handler.activate(click({ ctrlKey: true }), "https://example.com/docs");
    handler.activate(click({ ctrlKey: true }), "file:///etc/passwd");
    handler.activate(click({ ctrlKey: true }), "javascript:alert(1)");
    handler.activate(click({ ctrlKey: true }), "not a url");

    expect(opened).toEqual(["https://example.com/docs"]);
  });
});

describe("moving between panes", () => {
  it("keeps the terminal and its session when it is taken off the screen", async () => {
    const { controller, container, terminal, creates, create } = await attached();

    controller.detach();
    expect(container.children).toHaveLength(0);
    expect(FakeResizeObserver.active).toEqual([]);
    // Output keeps being processed and acknowledged while it is hidden.
    create.handlers.onOutput(new Uint8Array(ACK_BATCH));
    terminal.parseWrites();

    const other = pane();
    controller.attach(other);

    expect(other.contains(terminal.element ?? null)).toBe(true);
    expect(FakeTerminal.instances).toHaveLength(1);
    expect(creates).toHaveLength(1);
    expect(terminal.focusCount).toBe(2);
  });

  it("stops listening to its session when the terminal is disposed of", async () => {
    const { controller, terminal, create, acks, writes } = await attached();

    controller.dispose();

    expect(terminal.disposed).toBe(true);
    // Nothing reaches a terminal that is gone, and it says nothing more.
    const written = terminal.writes.length;
    create.handlers.onOutput(bytes("late"));
    create.handlers.onExit({ code: 0, signal: null });
    terminal.type("x");
    expect(terminal.writes).toHaveLength(written);
    expect(acks).toEqual([]);
    expect(writes).toEqual([]);
    // It cannot be started again either.
    controller.start();
    expect(controller.getStatus()).toEqual({ kind: "running", terminal: 1 });
  });

  it("ignores a session that only finishes starting after the terminal is gone", async () => {
    const { controller, create, acks } = await attached({ leaveStarting: true });
    create.handlers.onOutput(new Uint8Array(ACK_BATCH));

    controller.dispose();
    await create.started();

    expect(controller.getStatus()).toEqual({ kind: "starting" });
    expect(acks).toEqual([]);
  });
});

describe("a session that already runs (the page was reloaded)", () => {
  function reattaching() {
    const fake = fakeBackend();
    const controller = new TerminalController({
      sessionId: "s-live",
      autoStart: "reattach",
      backend: fake.backend,
      platform: "linux",
    });
    return { ...fake, controller };
  }

  it("is attached to, never started again", async () => {
    const { controller, attaches, creates, acks } = reattaching();

    controller.attach(pane());

    expect(creates).toHaveLength(0);
    expect(attaches.map((call) => [call.session, call.size])).toEqual([
      ["s-live", { cols: 80, rows: 24 }],
    ]);
    const attach = attaches[0];
    if (!attach) throw new Error("not attached");
    // The replay of its recent output repaints the terminal.
    attach.handlers.onOutput(bytes("before the reload"));
    await attach.started();
    expect(controller.getStatus()).toEqual({ kind: "running", terminal: 1 });
    expect(FakeTerminal.last.text).toContain("before the reload");
    expect(FakeTerminal.last.text).not.toContain("restored");
    attach.handlers.onOutput(new Uint8Array(ACK_BATCH));
    FakeTerminal.last.parseWrites();
    expect(acks.at(-1)?.id).toBe(1);
  });

  it("sends what is typed while it attaches once it is attached", async () => {
    const { controller, attaches, writes } = reattaching();
    controller.attach(pane());

    FakeTerminal.last.type("y");
    expect(writes).toEqual([]);
    await attaches[0]?.started();

    expect(writes.map((write) => write.data)).toEqual(["y"]);
  });

  it("says so when the session stopped meanwhile, and Enter resumes it", async () => {
    const { controller, attaches, creates } = reattaching();
    controller.attach(pane());

    await attaches[0]?.failed({ command: "session_attach", kind: "notFound", message: "" });

    expect(controller.getStatus()).toEqual({ kind: "dormant" });
    expect(FakeTerminal.last.text).toContain(
      "This session is not running. Press Enter to start it.",
    );
    FakeTerminal.last.type("\r");
    expect(creates[0]?.options.mode).toBe("resume");
  });

  it("can be attached to before it is shown", () => {
    const { controller, attaches } = reattaching();

    controller.reattach();
    controller.reattach();

    expect(attaches).toHaveLength(1);
  });
});

describe("a session whose process ended before its terminal existed", () => {
  it("says how it ended, and Enter starts it again", () => {
    const fake = fakeBackend();
    const controller = new TerminalController({
      sessionId: "s-ended",
      autoStart: { exited: { code: 4, signal: null, failure: null } },
      backend: fake.backend,
      platform: "linux",
    });

    controller.attach(pane());

    expect(fake.creates).toHaveLength(0);
    expect(FakeTerminal.last.text).toContain(
      "The process exited with code 4. Press Enter to restart.",
    );
    FakeTerminal.last.type("\r");
    expect(fake.creates[0]?.options.mode).toBe("fresh");
  });
});

describe("off the screen", () => {
  it("keeps parsing and acknowledging the output of a hidden terminal", async () => {
    const { controller, terminal, create, acks } = await attached();

    controller.detach();
    create.handlers.onOutput(new Uint8Array(ACK_BATCH));
    create.handlers.onOutput(new Uint8Array(ACK_BATCH));
    terminal.parseWrites();

    // The program is never held back because its terminal is not shown.
    expect(acks).toEqual([
      { id: 1, bytes: ACK_BATCH },
      { id: 1, bytes: ACK_BATCH },
    ]);
  });

  it("lets go of its WebGL context while hidden and makes one when shown again", async () => {
    const { controller } = await attached();
    const first = FakeWebglAddon.instances[0];

    controller.detach();
    expect(first?.disposed).toBe(true);
    expect(controller.renderer).toBe("dom");

    controller.attach(pane());
    expect(FakeWebglAddon.instances).toHaveLength(2);
    expect(controller.renderer).toBe("webgl");
  });

  it("does not try WebGL again when it was not available", async () => {
    FakeWebglAddon.unavailable = true;
    const { controller } = await attached();

    controller.detach();
    controller.attach(pane());

    expect(FakeWebglAddon.instances).toHaveLength(1);
    expect(controller.renderer).toBe("dom");
  });

  it("starts a session before it is shown, at the size it was given", async () => {
    const fake = fakeBackend();
    const controller = new TerminalController({
      sessionId: "s-hidden",
      autoStart: "dormant",
      backend: fake.backend,
      platform: "linux",
      size: { cols: 120, rows: 40 },
    });

    controller.startDetached("resume");
    expect(fake.creates[0]?.options).toEqual({ mode: "resume", cols: 120, rows: 40 });
    await fake.creates[0]?.started();
    expect(controller.getStatus()).toEqual({ kind: "running", terminal: 1 });

    // Shown later, it says nothing about being restored and starts nothing.
    controller.attach(pane());
    expect(fake.creates).toHaveLength(1);
    expect(FakeTerminal.last.text).not.toContain("restored");
  });

  it("takes the keyboard when shown only if asked to", () => {
    const fake = fakeBackend();
    const controller = new TerminalController({
      sessionId: SESSION,
      backend: fake.backend,
      platform: "linux",
    });

    controller.attach(pane(), { focus: false });

    expect(FakeTerminal.last.focusCount).toBe(0);
  });
});

describe("reporting to the workspace", () => {
  it("reports the latest title of the terminal, now and then", async () => {
    vi.useFakeTimers();
    const { terminal, titles } = await attached();

    // Claude Code animates a mark in front of its title for as long as it
    // works: the title never settles, and is reported all the same.
    terminal.setTitle("\u2802 Fix the login");
    vi.advanceTimersByTime(TITLE_REPORT_INTERVAL_MS - 1);
    terminal.setTitle("\u2810 Fix the login");
    expect(titles).toEqual([]);
    vi.advanceTimersByTime(1);
    expect(titles).toEqual([{ id: SESSION, title: "\u2810 Fix the login" }]);

    // Not every frame: one report per interval, with the title of the moment.
    for (let frame = 0; frame < 10; frame += 1) {
      terminal.setTitle(`${String(frame)} Fix the login`);
      vi.advanceTimersByTime(TITLE_REPORT_INTERVAL_MS / 5);
    }
    expect(titles.map((report) => report.title)).toEqual([
      "\u2810 Fix the login",
      "4 Fix the login",
      "9 Fix the login",
    ]);

    terminal.setTitle("");
    vi.advanceTimersByTime(TITLE_REPORT_INTERVAL_MS);
    expect(titles.at(-1)).toEqual({ id: SESSION, title: "" });
  });

  it("does not report a title after the terminal is gone", async () => {
    vi.useFakeTimers();
    const { controller, terminal, titles } = await attached();

    terminal.setTitle("late");
    controller.dispose();
    vi.advanceTimersByTime(TITLE_REPORT_INTERVAL_MS * 2);

    expect(titles).toEqual([]);
  });

  it("reports the folder a program says it is in", async () => {
    const { terminal, folders } = await attached();
    const report = terminal.oscHandlers.get(7);
    if (!report) throw new Error("no OSC 7 handler was registered");

    // The payload is passed on as it is: the Rust side knows how to read it.
    expect(report("file://host/Users/me/My%20Code")).toBe(true);

    expect(folders).toEqual([{ id: SESSION, cwd: "file://host/Users/me/My%20Code" }]);
  });
});

describe("a session that could not do its job", () => {
  it("says why in the words of the failure instead of an exit code", async () => {
    const { terminal, create } = await attached();

    create.handlers.onExit({
      code: 113,
      signal: null,
      failure: { kind: "claudeNotFound", message: "Claude Code is not installed." },
    });

    expect(terminal.text).toContain("Claude Code is not installed. Press Enter to try again.");
    expect(terminal.text).not.toContain("exited with code");
  });

  it("tries a failed start again the way it was asked", async () => {
    const fake = fakeBackend();
    const controller = new TerminalController({
      sessionId: SESSION,
      autoStart: "resume",
      backend: fake.backend,
      platform: "linux",
    });
    controller.attach(pane());
    await fake.creates[0]?.failed({
      command: "session_start",
      kind: "folderMissing",
      message: "the folder is gone",
    });
    expect(controller.getStatus()).toEqual({ kind: "failed", message: "the folder is gone" });

    FakeTerminal.last.type("\r");

    expect(fake.creates[1]?.options).toEqual({ mode: "resume", cols: 80, rows: 24 });
  });
});
