/**
 * Stand-ins for xterm.js and its addons, which need a real browser to lay
 * out and draw. They record what the terminal view does with them and let a
 * test play the part of xterm.js: finish parsing a write, report a key, lose
 * the WebGL context.
 *
 * Use them with `vi.mock`; see TerminalController.test.ts.
 */

type Listener<T> = (value: T) => void;

class Emitter<T> {
  private readonly listeners = new Set<Listener<T>>();

  readonly event = (listener: Listener<T>) => {
    this.listeners.add(listener);
    return {
      dispose: () => {
        this.listeners.delete(listener);
      },
    };
  };

  fire(value: T): void {
    for (const listener of [...this.listeners]) listener(value);
  }
}

interface Addon {
  activate?: (terminal: FakeTerminal) => void;
  dispose?: () => void;
}

export interface RecordedWrite {
  data: string | Uint8Array;
  /** Whether a test has let xterm.js "finish parsing" this write. */
  parsed: boolean;
  callback?: () => void;
}

export class FakeTerminal {
  static instances: FakeTerminal[] = [];

  static get last(): FakeTerminal {
    const terminal = FakeTerminal.instances.at(-1);
    if (!terminal) throw new Error("no terminal was created");
    return terminal;
  }

  cols = 80;
  rows = 24;
  element: HTMLElement | undefined;
  options: Record<string, unknown>;
  readonly addons: Addon[] = [];
  readonly writes: RecordedWrite[] = [];
  /** Everything that happened, in order, to check what came first. */
  readonly log: string[] = [];
  readonly pasted: string[] = [];
  readonly resizes: { cols: number; rows: number }[] = [];
  selection = "";
  focusCount = 0;
  disposed = false;
  keyHandler: ((event: KeyboardEvent) => boolean) | undefined;
  readonly oscHandlers = new Map<number, (payload: string) => boolean | Promise<boolean>>();

  private readonly versions = ["6"];
  private version = "6";
  readonly unicode = {
    versions: this.versions,
    register: (provider: { version: string }) => {
      this.versions.push(provider.version);
    },
  } as {
    versions: string[];
    register: (provider: { version: string }) => void;
    activeVersion: string;
  };

  readonly parser = {
    registerOscHandler: (
      ident: number,
      handler: (payload: string) => boolean | Promise<boolean>,
    ) => {
      this.oscHandlers.set(ident, handler);
      return { dispose: () => undefined };
    },
  };

  private readonly data = new Emitter<string>();
  private readonly binary = new Emitter<string>();
  private readonly resized = new Emitter<{ cols: number; rows: number }>();
  private readonly title = new Emitter<string>();
  readonly onData = this.data.event;
  readonly onBinary = this.binary.event;
  readonly onResize = this.resized.event;
  readonly onTitleChange = this.title.event;

  constructor(options: Record<string, unknown> = {}) {
    this.options = { ...options };
    if (typeof options.cols === "number") this.cols = options.cols;
    if (typeof options.rows === "number") this.rows = options.rows;
    Object.defineProperty(this.unicode, "activeVersion", {
      get: () => this.version,
      set: (version: string) => {
        if (!this.versions.includes(version)) {
          throw new Error(`unknown Unicode version ${version}`);
        }
        this.version = version;
        this.log.push(`unicode:${version}`);
      },
    });
    FakeTerminal.instances.push(this);
  }

  loadAddon(addon: Addon): void {
    this.addons.push(addon);
    addon.activate?.(this);
  }

  open(container: HTMLElement): void {
    if (this.element) throw new Error("a terminal can only be opened once");
    this.element = document.createElement("div");
    this.element.className = "xterm";
    container.appendChild(this.element);
    this.log.push("open");
  }

  write(data: string | Uint8Array, callback?: () => void): void {
    this.writes.push({ data, parsed: false, callback });
    this.log.push("write");
  }

  resize(cols: number, rows: number): void {
    this.cols = cols;
    this.rows = rows;
    this.resizes.push({ cols, rows });
    this.resized.fire({ cols, rows });
  }

  attachCustomKeyEventHandler(handler: (event: KeyboardEvent) => boolean): void {
    this.keyHandler = handler;
  }

  focus(): void {
    this.focusCount += 1;
  }

  getSelection(): string {
    return this.selection;
  }

  paste(text: string): void {
    this.pasted.push(text);
  }

  dispose(): void {
    this.disposed = true;
  }

  // --- What a test does in the name of xterm.js ----------------------------

  /** Finishes parsing every write so far, calling their callbacks. */
  parseWrites(): void {
    for (const write of this.writes) {
      if (write.parsed) continue;
      write.parsed = true;
      write.callback?.();
    }
  }

  /** The user typed `data`. */
  type(data: string): void {
    this.data.fire(data);
  }

  /** xterm.js reports input that is not text. */
  typeBinary(data: string): void {
    this.binary.fire(data);
  }

  /** A program set the title of the terminal (OSC 0 or 2). */
  setTitle(title: string): void {
    this.title.fire(title);
  }

  /** Sends a key event through the custom key handler. */
  key(
    type: "keydown" | "keypress" | "keyup",
    init: Omit<KeyboardEventInit, "keyCode" | "isComposing"> & {
      keyCode?: number;
      isComposing?: boolean;
    },
  ): {
    handledByXterm: boolean;
    event: KeyboardEvent;
  } {
    if (!this.keyHandler) throw new Error("no key handler was attached");
    const event = new KeyboardEvent(type, { cancelable: true, ...init });
    // happy-dom does not take these two from the init dictionary.
    Object.defineProperty(event, "isComposing", { value: init.isComposing ?? false });
    Object.defineProperty(event, "keyCode", { value: init.keyCode ?? 0 });
    return { handledByXterm: this.keyHandler(event), event };
  }

  /** Everything written to the terminal as text. */
  get text(): string {
    const decoder = new TextDecoder();
    return this.writes
      .map((write) => (typeof write.data === "string" ? write.data : decoder.decode(write.data)))
      .join("");
  }

  addon<T>(type: abstract new (...args: never[]) => T): T | undefined {
    return this.addons.find((addon): addon is T & Addon => addon instanceof type);
  }
}

export class FakeFitAddon {
  /** What the next measurements propose. */
  static proposed: { cols: number; rows: number } | undefined = { cols: 80, rows: 24 };

  proposeDimensions(): { cols: number; rows: number } | undefined {
    return FakeFitAddon.proposed;
  }

  fit(): void {
    throw new Error("the terminal view decides the size itself");
  }
}

export class FakeWebglAddon {
  static instances: FakeWebglAddon[] = [];
  /** Makes loading the addon fail, as when WebGL2 is not available. */
  static unavailable = false;

  disposed = false;
  private readonly contextLoss = new Emitter<void>();
  readonly onContextLoss = this.contextLoss.event;

  constructor() {
    FakeWebglAddon.instances.push(this);
  }

  /** Contexts lost with `WEBGL_lose_context`, over every instance. */
  static lostContexts = 0;
  canvas: HTMLCanvasElement | undefined;

  activate(terminal: FakeTerminal): void {
    if (FakeWebglAddon.unavailable) throw new Error("WebGL2 not supported");
    // The canvas of the renderer, with a context that can be lost.
    const canvas = document.createElement("canvas");
    const context = {
      getExtension: (name: string) =>
        name === "WEBGL_lose_context"
          ? {
              loseContext: () => {
                FakeWebglAddon.lostContexts += 1;
              },
            }
          : null,
    };
    Object.defineProperty(canvas, "getContext", { value: () => context });
    terminal.element?.appendChild(canvas);
    this.canvas = canvas;
  }

  dispose(): void {
    this.disposed = true;
    this.canvas?.remove();
  }

  /** The browser took the WebGL context away. */
  loseContext(): void {
    this.contextLoss.fire();
  }
}

export class FakeUnicodeGraphemesAddon {
  activate(terminal: FakeTerminal): void {
    terminal.unicode.register({ version: "15" });
    terminal.unicode.register({ version: "15-graphemes" });
  }
}

export class FakeWebLinksAddon {
  constructor(readonly handler: (event: MouseEvent, uri: string) => void) {}
}

/** A `ResizeObserver` that fires when a test says so. */
export class FakeResizeObserver {
  static instances: FakeResizeObserver[] = [];

  readonly observed: Element[] = [];
  disconnected = false;

  constructor(private readonly callback: () => void) {
    FakeResizeObserver.instances.push(this);
  }

  observe(element: Element): void {
    this.observed.push(element);
  }

  unobserve(): void {
    // Not used by the terminal view.
  }

  disconnect(): void {
    this.disconnected = true;
  }

  /** The observed element changed size. */
  trigger(): void {
    if (!this.disconnected) this.callback();
  }

  static get active(): FakeResizeObserver[] {
    return FakeResizeObserver.instances.filter((observer) => !observer.disconnected);
  }
}

/** Forgets everything the fakes recorded. Call it before each test. */
export function resetXtermMocks(): void {
  FakeTerminal.instances = [];
  FakeWebglAddon.instances = [];
  FakeWebglAddon.unavailable = false;
  FakeWebglAddon.lostContexts = 0;
  FakeFitAddon.proposed = { cols: 80, rows: 24 };
  FakeResizeObserver.instances = [];
}
