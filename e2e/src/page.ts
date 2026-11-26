/**
 * The code of the suite that runs inside the page of the application.
 *
 * `pageLibrary` is sent to the page as source text and called there once
 * (see `Terminal.install`). It must therefore be self-contained: it can use
 * nothing from this module or any other, only what it declares itself and
 * what the page offers. The types below describe what crosses the boundary,
 * as JSON.
 */

/** One key going down, up, or both ("press"), as a browser reports it. */
export interface KeyEvent {
  type: "down" | "up" | "press";
  /** `KeyboardEvent.key`: the character or the name of the key. */
  key: string;
  /** `KeyboardEvent.code`: the physical key, such as `KeyA` or `AltRight`. */
  code: string;
  /** The legacy `keyCode`, which xterm.js still decides by. */
  keyCode: number;
  /** `KeyboardEvent.location`: 1 for a left modifier, 2 for a right one. */
  location?: number;
  shift?: boolean;
  ctrl?: boolean;
  alt?: boolean;
  meta?: boolean;
  /**
   * The text the key types, which a webview announces with a `keypress`
   * event after the `keydown`, unless a handler cancelled it.
   */
  text?: string;
}

/** The state of the session behind the terminal, as the application sees it. */
export interface TerminalStatus {
  kind: "idle" | "starting" | "running" | "exited" | "failed";
  sessionId?: number;
  exit?: { code: number | null; signal: string | null };
  message?: string;
}

export interface TerminalState {
  status: TerminalStatus;
  cols: number;
  rows: number;
  /** What draws the terminal. */
  renderer: "webgl" | "dom";
  /** Whether the program in the terminal asked for bracketed paste. */
  bracketedPaste: boolean;
  /** Number of lines in the buffer, scrollback included. */
  bufferLength: number;
}

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface PageLayout {
  /** The pane of the terminal, in CSS pixels. */
  terminal: Rect;
  /** The screen xterm.js draws on, in CSS pixels. */
  screen: Rect;
  /** Width of the page, in CSS pixels. */
  viewportWidth: number;
  /** The background colour of the terminal pane, as CSS reports it. */
  background: string;
}

/** What xterm.js has drawn, measured where it draws. */
export interface Drawing {
  renderer: "webgl" | "dom";
  /**
   * With WebGL: the pixels of its drawing buffer that are not the
   * background, right after a frame was drawn. With the DOM renderer: the
   * characters, other than blanks, in the elements of its rows.
   */
  drawn: number;
}

export type PageLibrary = ReturnType<typeof pageLibrary>;

/** The name the library is installed under in the page. */
export const PAGE_LIBRARY_NAME = "__TERMX_E2E_PAGE__";

export function pageLibrary() {
  interface BufferLine {
    isWrapped: boolean;
    translateToString(trimRight?: boolean): string;
  }
  interface XtermTerminal {
    cols: number;
    rows: number;
    onRender(listener: () => void): { dispose(): void };
    refresh(start: number, end: number): void;
    textarea?: HTMLTextAreaElement;
    element?: HTMLElement;
    modes: { bracketedPasteMode: boolean };
    buffer: { active: { length: number; getLine(index: number): BufferLine | undefined } };
  }
  interface Controller {
    terminal: XtermTerminal;
    renderer: "webgl" | "dom";
    getStatus(): TerminalStatus;
    focus(): void;
  }
  interface Hook {
    store: {
      getSnapshot(): { activeKey: string | null };
      get(key: string): Controller | undefined;
    };
  }

  function controller(): Controller {
    const hook = (window as unknown as { __TERMX_E2E__?: true | Hook }).__TERMX_E2E__;
    if (hook === undefined || hook === true) {
      throw new Error("this build of termx does not expose its terminals to the tests");
    }
    const key = hook.store.getSnapshot().activeKey;
    const active = key === null ? undefined : hook.store.get(key);
    if (!active) throw new Error("the window has no terminal yet");
    return active;
  }

  function textarea(): HTMLTextAreaElement {
    const element = controller().terminal.textarea;
    if (!element) throw new Error("the terminal is not on screen yet");
    return element;
  }

  function rectOf(element: Element): Rect {
    const { x, y, width, height } = element.getBoundingClientRect();
    return { x, y, width, height };
  }

  function dispatchKey(
    target: HTMLElement,
    type: "keydown" | "keypress" | "keyup",
    key: KeyEvent,
    codes: { keyCode: number; charCode: number },
  ): boolean {
    const event = new KeyboardEvent(type, {
      key: key.key,
      code: key.code,
      location: key.location ?? 0,
      shiftKey: key.shift ?? false,
      ctrlKey: key.ctrl ?? false,
      altKey: key.alt ?? false,
      metaKey: key.meta ?? false,
      bubbles: true,
      cancelable: true,
      composed: true,
    });
    // The constructor cannot set the legacy fields, which are what xterm.js
    // reads, as every terminal written for the web still does.
    const which = type === "keypress" ? codes.charCode : codes.keyCode;
    Object.defineProperty(event, "keyCode", { get: () => which });
    Object.defineProperty(event, "which", { get: () => which });
    Object.defineProperty(event, "charCode", { get: () => codes.charCode });
    return target.dispatchEvent(event);
  }

  return {
    /** Whether the application has put a terminal on screen. */
    isReady(): boolean {
      try {
        return controller().terminal.textarea !== undefined;
      } catch {
        return false;
      }
    },

    state(): TerminalState {
      const active = controller();
      const terminal = active.terminal;
      return {
        status: active.getStatus(),
        cols: terminal.cols,
        rows: terminal.rows,
        renderer: active.renderer,
        bracketedPaste: terminal.modes.bracketedPasteMode,
        bufferLength: terminal.buffer.active.length,
      };
    },

    /**
     * The last `tail` lines of the terminal, scrollback included, read from
     * the buffer of xterm.js. A line the terminal wrapped is one line here,
     * and trailing blanks are dropped.
     */
    lines(tail: number): string[] {
      const buffer = controller().terminal.buffer.active;
      const lines: string[] = [];
      // A wrapped line can start before the range: begin a little earlier.
      const first = Math.max(0, buffer.length - tail - 50);
      for (let index = first; index < buffer.length; index += 1) {
        const line = buffer.getLine(index);
        if (!line) continue;
        const text = line.translateToString(true);
        if (line.isWrapped && lines.length > 0) {
          lines[lines.length - 1] = `${lines[lines.length - 1] ?? ""}${text}`;
        } else {
          lines.push(text);
        }
      }
      return lines.slice(-tail);
    },

    /**
     * Delivers key events to the terminal, in order, the way a webview
     * does: a `keydown`, then a `keypress` if the key types text and no
     * handler cancelled the `keydown`, then a `keyup`.
     */
    keys(events: KeyEvent[]): void {
      const target = textarea();
      for (const key of events) {
        const codes = { keyCode: key.keyCode, charCode: 0 };
        if (key.type === "up") {
          dispatchKey(target, "keyup", key, codes);
          continue;
        }
        const proceed = dispatchKey(target, "keydown", key, codes);
        if (proceed && key.text !== undefined && key.text !== "") {
          const charCode = key.text.codePointAt(0) ?? 0;
          dispatchKey(target, "keypress", key, { keyCode: charCode, charCode });
        }
        if (key.type === "press") dispatchKey(target, "keyup", key, codes);
      }
    },

    /**
     * Pastes `text` the way the webview does for Cmd+V, the Edit menu or
     * the context menu: with a `paste` event that carries the text.
     * Returns whether the application took the event.
     */
    paste(text: string): boolean {
      const data = new DataTransfer();
      data.setData("text/plain", text);
      let event: Event = new ClipboardEvent("paste", {
        clipboardData: data,
        bubbles: true,
        cancelable: true,
      });
      if ((event as ClipboardEvent).clipboardData?.getData("text/plain") !== text) {
        // A webview whose constructor ignores the data.
        event = new Event("paste", { bubbles: true, cancelable: true });
        Object.defineProperty(event, "clipboardData", { value: data });
      }
      return !textarea().dispatchEvent(event);
    },

    layout(): PageLayout {
      const pane = document.querySelector(".terminal-host");
      const screen = document.querySelector(".terminal-host .xterm-screen");
      if (!pane || !screen) throw new Error("the terminal is not on screen yet");
      return {
        terminal: rectOf(pane),
        screen: rectOf(screen),
        viewportWidth: window.innerWidth,
        background: getComputedStyle(pane).backgroundColor,
      };
    },

    /**
     * Measures what xterm.js has drawn. A screenshot shows what reaches
     * the screen, but not on every platform (see `specs/launch.e2e.ts`);
     * this asks the renderer itself.
     */
    drawing(background: [number, number, number], tolerance: number): Promise<Drawing> {
      const active = controller();
      const terminal = active.terminal;
      if (active.renderer === "dom") {
        const rows = terminal.element?.querySelector(".xterm-rows")?.textContent ?? "";
        return Promise.resolve({ renderer: "dom", drawn: rows.replace(/\s/g, "").length });
      }
      return new Promise((resolve, reject) => {
        // The drawing buffer of WebGL only holds a frame until it is shown:
        // it has to be read in the same turn the frame is drawn in.
        const subscription = terminal.onRender(() => {
          subscription.dispose();
          try {
            // The canvas of the WebGL renderer, next to the one of the links.
            const canvases = Array.from(
              document.querySelectorAll<HTMLCanvasElement>(
                ".terminal-host .xterm-screen > canvas:not(.xterm-link-layer)",
              ),
            );
            let gl: WebGL2RenderingContext | null = null;
            for (const canvas of canvases) {
              gl = canvas.getContext("webgl2");
              if (gl) break;
            }
            if (!gl) throw new Error("the terminal has no WebGL canvas");
            const width = gl.drawingBufferWidth;
            const height = gl.drawingBufferHeight;
            const pixels = new Uint8Array(width * height * 4);
            gl.readPixels(0, 0, width, height, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
            let drawn = 0;
            for (let offset = 0; offset < pixels.length; offset += 4) {
              const difference = Math.max(
                Math.abs((pixels[offset] ?? 0) - background[0]),
                Math.abs((pixels[offset + 1] ?? 0) - background[1]),
                Math.abs((pixels[offset + 2] ?? 0) - background[2]),
              );
              if (difference > tolerance) drawn += 1;
            }
            resolve({ renderer: "webgl", drawn });
          } catch (error) {
            reject(error instanceof Error ? error : new Error(String(error)));
          }
        });
        terminal.refresh(0, terminal.rows - 1);
      });
    },

    /** Throws an error nothing catches, outside the script that asks for it. */
    throwUncaught(message: string): void {
      setTimeout(() => {
        throw new Error(message);
      }, 0);
    },

    /** The text on the system clipboard, read the way the application reads it. */
    readClipboard(): Promise<string> {
      const internals = (
        window as unknown as {
          __TAURI_INTERNALS__: { invoke(command: string): Promise<string> };
        }
      ).__TAURI_INTERNALS__;
      return internals.invoke("plugin:clipboard-manager|read_text");
    },

    /**
     * Resolves on the next turn of the event loop of the page: an answer
     * means that the page is not frozen.
     */
    ping(): Promise<void> {
      return new Promise((resolve) => {
        setTimeout(resolve, 0);
      });
    },
  };
}
