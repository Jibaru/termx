import { FitAddon } from "@xterm/addon-fit";
import { UnicodeGraphemesAddon } from "@xterm/addon-unicode-graphemes";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import { type ITerminalOptions, Terminal } from "@xterm/xterm";

import { reportError } from "../lib/errorReporting";
import {
  ackSession,
  closeSession,
  type CreatedSession,
  type CreateSessionOptions,
  createSession,
  describeError,
  logFrontendEvent,
  openExternalUrl,
  readClipboardText,
  resizeSession,
  type SessionExit,
  type SessionHandlers,
  writeClipboardText,
  writeSession,
} from "../lib/ipc";
import { detectPlatform, type Platform } from "../lib/platform";
import { DEFAULT_TERMINAL_CONFIG, type TerminalConfig } from "./config";
import { isComposing, mapKey, OptionKeys, sanitizePaste } from "./keymap";
import { clipboardTextFromOsc52 } from "./osc52";

/**
 * One terminal: an xterm.js instance, the session behind it, and everything
 * that connects the two.
 *
 * ```text
 * session channel --bytes--> terminal.write(bytes, done) --done--> ackSession
 * keys, paste     --text---> input queue ----------------------> writeSession
 * pane resized    --------> debounced fit --> terminal.resize --> resizeSession
 * ```
 *
 * The controller lives outside React. A component only gives it an element
 * to draw in (`attach`) and takes it away again (`detach`); the terminal and
 * its session survive in between, which is what lets several terminals exist
 * while only the active one is on screen.
 */

/** How long the size of the pane must be stable before the terminal is refitted. */
export const RESIZE_DEBOUNCE_MS = 50;

/**
 * Smallest pane the terminal is fitted to, in CSS pixels. A pane that is
 * hidden or still being laid out measures nothing, or a few pixels: fitting
 * then would resize the session to a couple of columns and make every
 * program in it redraw for nothing.
 */
export const MIN_FIT_WIDTH_PX = 48;
export const MIN_FIT_HEIGHT_PX = 24;

/**
 * Used until the session says how to batch acknowledgements. Must not be
 * larger than the low-water mark of the flow control in termx-core.
 */
const DEFAULT_ACK_BATCH_BYTES = 8 * 1024;

/**
 * Sent to the terminal before the notice that the process ended, to undo
 * what a program that died without cleaning up may have left behind: the
 * alternate screen, mouse reporting, bracketed paste, a synchronized update
 * in progress, application cursor keys, a hidden cursor, colours.
 *
 * The alternate screen is left with mode 1047, not the usual 1049: 1049 also
 * restores the saved cursor position, which would move the notice to
 * wherever the cursor was saved last when no alternate screen was in use.
 */
const RESTORE_TERMINAL =
  "\x1b[?1047l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?2004l\x1b[?2026l\x1b[?1l\x1b>\x1b[?25h\x1b[0m";

/** The state of the session behind a terminal. */
export type TerminalStatus =
  /** No session was started yet: the terminal was never shown. */
  | { kind: "idle" }
  | { kind: "starting" }
  | { kind: "running"; sessionId: number }
  /** The process ended. Enter starts a new one. */
  | { kind: "exited"; exit: SessionExit }
  /** The session could not be started. Enter tries again. */
  | { kind: "failed"; message: string };

/** What a controller needs from the outside. Tests replace it. */
export interface TerminalBackend {
  createSession: (
    options: CreateSessionOptions,
    handlers: SessionHandlers,
  ) => Promise<CreatedSession>;
  writeSession: (id: number, data: string, binary?: boolean) => Promise<void>;
  resizeSession: (id: number, cols: number, rows: number) => Promise<void>;
  ackSession: (id: number, bytes: number) => Promise<void>;
  closeSession: (id: number) => Promise<void>;
  openExternalUrl: (url: string) => Promise<void>;
  readClipboardText: () => Promise<string>;
  writeClipboardText: (text: string) => Promise<void>;
}

const TAURI_BACKEND: TerminalBackend = {
  createSession,
  writeSession,
  resizeSession,
  ackSession,
  closeSession,
  openExternalUrl,
  readClipboardText,
  writeClipboardText,
};

export interface TerminalControllerOptions {
  config?: TerminalConfig;
  platform?: Platform;
  backend?: TerminalBackend;
  /** What the session runs. The login shell in the home directory by default. */
  session?: Omit<CreateSessionOptions, "cols" | "rows">;
}

interface PendingInput {
  data: string;
  binary: boolean;
}

/** The message shown in the terminal when its process has ended. */
export function exitNotice(exit: SessionExit): string {
  let what: string;
  if (exit.signal !== null) {
    what = `The process was terminated (${exit.signal}).`;
  } else if (exit.code !== null) {
    what = `The process exited with code ${String(exit.code)}.`;
  } else {
    what = "The process exited.";
  }
  return notice(`${what} Press Enter to restart.`);
}

/** The message shown in the terminal when its session could not be started. */
export function failureNotice(message: string): string {
  return notice(`Could not start the shell: ${message}. Press Enter to try again.`);
}

/** A line of termx's own, set apart from the output of programs. */
function notice(text: string): string {
  return `${RESTORE_TERMINAL}\r\n\x1b[7m termx \x1b[0m ${text}\r\n`;
}

function xtermOptions(config: TerminalConfig): ITerminalOptions {
  return {
    // Needed for the Unicode provider and the parser hooks.
    allowProposedApi: true,
    allowTransparency: false,
    cursorBlink: config.cursorBlink,
    cursorStyle: config.cursorStyle,
    cursorInactiveStyle: "outline",
    fontFamily: config.fontFamily,
    fontSize: config.fontSize,
    lineHeight: config.lineHeight,
    letterSpacing: config.letterSpacing,
    // On macOS this is switched per key: see `syncOptionKey`.
    macOptionIsMeta: config.optionAsMeta === "both",
    macOptionClickForcesSelection: true,
    minimumContrastRatio: config.minimumContrastRatio,
    scrollback: config.scrollback,
    theme: { ...config.theme },
  };
}

export class TerminalController {
  readonly terminal: Terminal;

  private readonly config: TerminalConfig;
  private readonly platform: Platform;
  private readonly backend: TerminalBackend;
  private readonly sessionOptions: Omit<CreateSessionOptions, "cols" | "rows">;

  private readonly fitAddon = new FitAddon();
  private webglAddon: WebglAddon | null = null;
  private readonly optionKeys = new OptionKeys();

  private status: TerminalStatus = { kind: "idle" };
  private readonly listeners = new Set<() => void>();

  /**
   * Counts the sessions this terminal has had. What a previous session
   * still delivers, or a start that resolves after the terminal moved on,
   * is recognised by its number and ignored.
   */
  private generation = 0;
  private sessionId: number | null = null;
  private ackBatchBytes = DEFAULT_ACK_BATCH_BYTES;
  /** Bytes processed by xterm.js and not acknowledged yet. */
  private unacknowledged = 0;
  /** The size the session was last told. */
  private sessionSize: { cols: number; rows: number } | null = null;

  private readonly pendingInput: PendingInput[] = [];
  private writing = false;

  private container: HTMLElement | null = null;
  private opened = false;
  private disposed = false;
  private resizeObserver: ResizeObserver | null = null;
  private resizeTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(options: TerminalControllerOptions = {}) {
    this.config = options.config ?? DEFAULT_TERMINAL_CONFIG;
    this.platform = options.platform ?? detectPlatform();
    this.backend = options.backend ?? TAURI_BACKEND;
    this.sessionOptions = options.session ?? {};

    this.terminal = new Terminal({
      ...xtermOptions(this.config),
      // OSC 8 hyperlinks. The default handler asks for confirmation in a
      // dialog and opens a browser window, neither of which exists here.
      linkHandler: {
        activate: (event, uri) => {
          this.openLink(event, uri);
        },
      },
    });

    // The Unicode version decides how wide each character is, and xterm.js
    // stores that width in the cells as it writes them: it has to be
    // settled before the first byte of output.
    this.terminal.loadAddon(new UnicodeGraphemesAddon());
    this.terminal.unicode.activeVersion = this.config.unicodeVersion;

    this.terminal.loadAddon(this.fitAddon);
    this.terminal.loadAddon(
      new WebLinksAddon((event, uri) => {
        this.openLink(event, uri);
      }),
    );

    this.terminal.attachCustomKeyEventHandler((event) => this.handleKey(event));
    this.terminal.onData((data) => {
      this.handleInput(data, false);
    });
    this.terminal.onBinary((data) => {
      this.handleInput(data, true);
    });
    this.terminal.onResize(() => {
      this.syncSessionSize();
    });
    this.terminal.parser.registerOscHandler(52, (payload) => {
      this.handleClipboardRequest(payload);
      return true;
    });
  }

  /** The state of the session behind this terminal. */
  getStatus = (): TerminalStatus => this.status;

  /** Calls `listener` whenever the status changes. Returns how to stop. */
  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  /**
   * Shows the terminal in `container`, and starts its session if it has
   * none yet. The session is started here rather than earlier so that it is
   * created at the size of the pane: a shell started at 80 columns and
   * resized at once would draw its first prompt twice.
   */
  attach(container: HTMLElement): void {
    if (this.disposed) return;
    this.detach();
    this.container = container;

    if (this.opened) {
      // xterm.js can only be opened once; afterwards its element is moved.
      if (this.terminal.element) container.appendChild(this.terminal.element);
    } else {
      this.terminal.open(container);
      this.opened = true;
      this.enableWebgl();
    }

    this.fit();
    this.resizeObserver = new ResizeObserver(() => {
      this.scheduleFit();
    });
    this.resizeObserver.observe(container);
    // In the capture phase, before xterm.js sees the event: see `handlePaste`.
    container.addEventListener("paste", this.handlePaste, true);
    window.addEventListener("blur", this.handleWindowBlur);
    this.terminal.focus();

    if (this.status.kind === "idle") this.start();
  }

  /** Takes the terminal off the screen. Its session keeps running. */
  detach(): void {
    this.resizeObserver?.disconnect();
    this.resizeObserver = null;
    if (this.resizeTimer !== null) {
      clearTimeout(this.resizeTimer);
      this.resizeTimer = null;
    }
    const element = this.terminal.element;
    if (this.container && element?.parentElement === this.container) {
      this.container.removeChild(element);
    }
    this.container?.removeEventListener("paste", this.handlePaste, true);
    window.removeEventListener("blur", this.handleWindowBlur);
    this.container = null;
  }

  /** Gives the keyboard to the terminal. */
  focus(): void {
    this.terminal.focus();
  }

  /** Closes the session and releases the terminal. It cannot be used again. */
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.generation += 1;
    this.detach();
    const id = this.sessionId;
    this.sessionId = null;
    if (id !== null) void this.backend.closeSession(id).catch(ignore);
    this.listeners.clear();
    this.terminal.dispose();
  }

  // --- Session -------------------------------------------------------------

  private start(): void {
    const generation = ++this.generation;
    this.sessionId = null;
    this.unacknowledged = 0;
    this.pendingInput.length = 0;
    const size = { cols: this.terminal.cols, rows: this.terminal.rows };
    this.sessionSize = size;
    this.setStatus({ kind: "starting" });

    this.backend
      .createSession(
        { ...this.sessionOptions, ...size },
        {
          onOutput: (bytes) => {
            this.handleOutput(generation, bytes);
          },
          onExit: (exit) => {
            this.handleExit(generation, exit);
          },
        },
      )
      .then((created) => {
        if (generation !== this.generation) {
          // The terminal was disposed of while its session was starting.
          void this.backend.closeSession(created.id).catch(ignore);
          return;
        }
        if (created.windowsBuildNumber !== null) {
          this.terminal.options.windowsPty = {
            backend: "conpty",
            buildNumber: created.windowsBuildNumber,
          };
        }
        this.ackBatchBytes = created.ackBatchBytes;
        // A process that ended at once has reported its exit already: there
        // is no session left to talk to.
        if (this.status.kind !== "starting") return;

        this.sessionId = created.id;
        this.setStatus({ kind: "running", sessionId: created.id });
        // Whatever happened while the session was starting.
        this.acknowledge();
        this.syncSessionSize();
        this.flushInput();
      })
      .catch((error: unknown) => {
        if (generation !== this.generation) return;
        const message = describeError(error);
        this.setStatus({ kind: "failed", message });
        this.terminal.write(failureNotice(message));
      });
  }

  private handleOutput(generation: number, bytes: Uint8Array): void {
    if (generation !== this.generation) return;
    // Acknowledged once xterm.js has parsed the bytes, not when they
    // arrive: acknowledging on receipt would hide the backlog of xterm.js
    // from the flow control, which exists to bound exactly that.
    this.terminal.write(bytes, () => {
      if (generation !== this.generation) return;
      this.unacknowledged += bytes.length;
      this.acknowledge();
    });
  }

  private acknowledge(): void {
    if (this.sessionId === null || this.unacknowledged < this.ackBatchBytes) return;
    const bytes = this.unacknowledged;
    this.unacknowledged = 0;
    void this.backend.ackSession(this.sessionId, bytes).catch(ignore);
  }

  private handleExit(generation: number, exit: SessionExit): void {
    if (generation !== this.generation) return;
    this.sessionId = null;
    this.pendingInput.length = 0;
    this.setStatus({ kind: "exited", exit });
    // Written like any output, so it appears after everything the process
    // printed, which xterm.js may still be working through.
    this.terminal.write(exitNotice(exit));
  }

  private setStatus(status: TerminalStatus): void {
    this.status = status;
    for (const listener of this.listeners) listener();
  }

  // --- Input ---------------------------------------------------------------

  private handleInput(data: string, binary: boolean): void {
    if (this.disposed) return;
    if (this.status.kind === "exited" || this.status.kind === "failed") {
      // Only Enter does something in a terminal without a process.
      if (data.includes("\r")) this.start();
      return;
    }
    if (this.status.kind === "idle") return;

    // Adjacent input of the same kind is sent together.
    const last = this.pendingInput.at(-1);
    if (last?.binary === binary) {
      last.data += data;
    } else {
      this.pendingInput.push({ data, binary });
    }
    this.flushInput();
  }

  /**
   * Sends the queued input, one call at a time: the next one starts when
   * the previous one has returned, which keeps keys in the order they were
   * typed however the calls are scheduled on the other side. What is typed
   * in the meantime, or while the session is starting, waits in the queue.
   */
  private flushInput(): void {
    if (this.writing || this.sessionId === null) return;
    const input = this.pendingInput.shift();
    if (!input) return;

    this.writing = true;
    this.backend
      .writeSession(this.sessionId, input.data, input.binary)
      // A failed write is logged where it failed. The input is lost; the
      // rest must still go through.
      .catch(ignore)
      .finally(() => {
        this.writing = false;
        this.flushInput();
      });
  }

  /**
   * Makes xterm.js treat Option as Meta, or not, for the key at hand.
   *
   * xterm.js has one switch for both Option keys. With it on, Option+key
   * sends `ESC` and the key; with it off, the key is left to the system,
   * which types the alternate character of the layout or starts a dead key.
   * Setting the switch before each key lets the two Option keys differ,
   * while xterm.js keeps doing the encoding and the composition it is good
   * at.
   */
  private syncOptionKey(event: KeyboardEvent): void {
    if (this.platform !== "macos") return;
    this.optionKeys.track(event);
    // The switch only matters for the key Option modifies, so it is left
    // alone when an Option key itself moves, and during a composition.
    if (event.key === "Alt" || isComposing(event)) return;
    const meta = this.optionKeys.actsAsMeta(this.config.optionAsMeta);
    if (this.terminal.options.macOptionIsMeta !== meta) {
      this.terminal.options.macOptionIsMeta = meta;
    }
  }

  private readonly handleWindowBlur = (): void => {
    // A key released in another window is never reported here.
    this.optionKeys.reset();
  };

  private handleKey(event: KeyboardEvent): boolean {
    this.syncOptionKey(event);
    const action = mapKey(event, this.platform);
    if (action.kind === "default") return true;

    // The key is ours: xterm.js must not encode it and the webview must not
    // act on it. Only the key going down triggers the action.
    event.preventDefault();
    if (event.type !== "keydown") return false;

    switch (action.kind) {
      case "send":
        this.handleInput(action.data, false);
        break;
      case "copy":
        this.copySelection();
        break;
      case "paste":
        this.pasteClipboard();
        break;
    }
    return false;
  }

  private copySelection(): void {
    const selection = this.terminal.getSelection();
    if (selection === "") return;
    this.backend.writeClipboardText(selection).catch((error: unknown) => {
      reportError("terminal-copy", error);
    });
  }

  private pasteClipboard(): void {
    this.backend
      .readClipboardText()
      .then((text) => {
        this.paste(text);
      })
      .catch((error: unknown) => {
        reportError("terminal-paste", error);
      });
  }

  /**
   * Every paste the webview performs itself (Cmd+V, the Edit menu, the
   * context menu) arrives here before it reaches xterm.js, so that pasted
   * text is sanitised whatever way it comes in.
   */
  private readonly handlePaste = (event: ClipboardEvent): void => {
    event.preventDefault();
    event.stopPropagation();
    this.paste(event.clipboardData?.getData("text/plain") ?? "");
  };

  /**
   * Pastes `text` into the terminal. xterm.js wraps it in the bracketed
   * paste markers when the program asked for them, and turns its line
   * endings into carriage returns either way; the text itself must not
   * contain the end marker.
   */
  private paste(text: string): void {
    const clean = sanitizePaste(text);
    if (clean !== "" && !this.disposed) this.terminal.paste(clean);
  }

  /** A program asked to access the clipboard (OSC 52). */
  private handleClipboardRequest(payload: string): void {
    const text = clipboardTextFromOsc52(payload);
    if (text === null) return;
    this.backend.writeClipboardText(text).catch((error: unknown) => {
      reportError("terminal-osc52", error);
    });
  }

  /**
   * Opens a link of the terminal in the system browser when it is clicked
   * with Cmd (macOS) or Ctrl held, as in most terminals: a plain click
   * belongs to the program, or to the selection.
   */
  private openLink(event: MouseEvent, uri: string): void {
    const modifier = this.platform === "macos" ? event.metaKey : event.ctrlKey;
    if (!modifier) return;

    let url: URL;
    try {
      url = new URL(uri);
    } catch {
      return;
    }
    if (url.protocol !== "http:" && url.protocol !== "https:") return;
    this.backend.openExternalUrl(url.href).catch((error: unknown) => {
      reportError("terminal-link", error);
    });
  }

  // --- Size ----------------------------------------------------------------

  private scheduleFit(): void {
    if (this.resizeTimer !== null) clearTimeout(this.resizeTimer);
    this.resizeTimer = setTimeout(() => {
      this.resizeTimer = null;
      this.fit();
    }, RESIZE_DEBOUNCE_MS);
  }

  /** Resizes the terminal to its pane, if the pane has a usable size. */
  private fit(): void {
    const container = this.container;
    if (!container) return;
    if (container.clientWidth < MIN_FIT_WIDTH_PX || container.clientHeight < MIN_FIT_HEIGHT_PX) {
      return;
    }
    const proposed = this.fitAddon.proposeDimensions();
    if (!proposed || !Number.isFinite(proposed.cols) || !Number.isFinite(proposed.rows)) return;
    const cols = Math.max(2, Math.floor(proposed.cols));
    const rows = Math.max(1, Math.floor(proposed.rows));
    if (cols === this.terminal.cols && rows === this.terminal.rows) return;
    // Fires `onResize`, which tells the session.
    this.terminal.resize(cols, rows);
  }

  /** Tells the session the size of the terminal, if it does not know it. */
  private syncSessionSize(): void {
    if (this.sessionId === null) return;
    const { cols, rows } = this.terminal;
    if (this.sessionSize?.cols === cols && this.sessionSize.rows === rows) return;
    this.sessionSize = { cols, rows };
    void this.backend.resizeSession(this.sessionId, cols, rows).catch(ignore);
  }

  // --- Renderer ------------------------------------------------------------

  /**
   * Draws with WebGL when possible. Without it, and after its context is
   * lost (the GPU process restarted, the machine woke up...), xterm.js
   * draws with its DOM renderer, which is slower but works everywhere.
   */
  private enableWebgl(): void {
    if (this.config.renderer !== "webgl") return;
    try {
      const addon = new WebglAddon();
      addon.onContextLoss(() => {
        addon.dispose();
        if (this.webglAddon === addon) this.webglAddon = null;
        void logFrontendEvent({
          level: "warn",
          source: "terminal-renderer",
          message: "the WebGL context was lost; the terminal now uses the DOM renderer",
        });
      });
      this.terminal.loadAddon(addon);
      this.webglAddon = addon;
    } catch (error) {
      this.webglAddon = null;
      void logFrontendEvent({
        level: "warn",
        source: "terminal-renderer",
        message: `WebGL is not available; the terminal uses the DOM renderer (${
          error instanceof Error ? error.name : typeof error
        })`,
      });
    }
  }

  /** Which renderer draws the terminal. */
  get renderer(): "webgl" | "dom" {
    return this.webglAddon ? "webgl" : "dom";
  }
}

/** For calls whose failure was already logged by the IPC layer. */
function ignore(): void {
  // Nothing to do.
}
