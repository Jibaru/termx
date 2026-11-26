import { fileURLToPath } from "node:url";

import { control, enter, typing } from "./keys.ts";
import {
  type Drawing,
  type KeyEvent,
  PAGE_LIBRARY_NAME,
  type PageLayout,
  type PageLibrary,
  pageLibrary,
  type TerminalState,
} from "./page.ts";
import { CHANNEL_TOLERANCE, parseColour } from "./pixels.ts";
import { DEFAULT_TIMEOUT_MS, waitFor } from "./wait.ts";
import type { WebDriver } from "./webdriver.ts";

/** The script of the programs the tests run inside the terminal. */
const HELPER_PATH = fileURLToPath(new URL("../helpers/helper.mjs", import.meta.url));

/** How many lines a test looks at by default: more than a screen. */
const DEFAULT_TAIL = 200;

type Awaitable<T> = T | Promise<T>;

/** A program of `helper.mjs` running in the terminal. */
export interface HelperRun {
  /** What tells the output of this run apart from every other. */
  id: string;
}

/**
 * The terminal of the application, as a test uses it: it types and pastes
 * like a person would, and reads what xterm.js shows from its buffer.
 */
export class Terminal {
  private nextId = 1;

  constructor(private readonly driver: WebDriver) {}

  /** Sends the page its part of the suite. Done by the first call for a page. */
  private async install(): Promise<void> {
    await this.driver.execute(
      `window[arguments[0]] = {
         functions: (${pageLibrary.toString()})(),
         settled: {},
         nextPending: 1,
       };
       return true;`,
      PAGE_LIBRARY_NAME,
    );
  }

  /**
   * Calls a function of the page library and returns what it returns, or
   * what its promise resolves to.
   *
   * Only the synchronous scripts of WebDriver are used, to depend on as
   * little of the driver as possible: a promise is left in the page and
   * asked about until it has settled.
   */
  private async call<K extends keyof PageLibrary>(
    name: K,
    ...args: Parameters<PageLibrary[K]>
  ): Promise<Awaited<ReturnType<PageLibrary[K]>>> {
    type Outcome = { value: unknown } | { error: string } | { pending: number };
    const invoke = () =>
      this.driver.execute<Outcome | { missing: true }>(
        `const library = window[arguments[0]];
         if (!library) return { missing: true };
         const describe = (error) => String(error && error.stack ? error.stack : error);
         try {
           const value = library.functions[arguments[1]].apply(null, arguments[2]);
           if (value && typeof value.then === "function") {
             const id = library.nextPending++;
             value.then(
               (resolved) => { library.settled[id] = { value: resolved === undefined ? null : resolved }; },
               (error) => { library.settled[id] = { error: describe(error) }; },
             );
             return { pending: id };
           }
           return { value: value === undefined ? null : value };
         } catch (error) {
           return { error: describe(error) };
         }`,
        PAGE_LIBRARY_NAME,
        name,
        args,
      );
    let outcome = await invoke();
    if ("missing" in outcome) {
      // The first call for a page, also after it was loaded again.
      await this.install();
      outcome = await invoke();
      if ("missing" in outcome) throw new Error("the page does not keep the library of the suite");
    }
    if ("pending" in outcome) {
      const id = outcome.pending;
      outcome = await waitFor(
        () =>
          this.driver.execute<Outcome | null>(
            `const settled = window[arguments[0]].settled;
             const outcome = settled[arguments[1]];
             delete settled[arguments[1]];
             return outcome === undefined ? null : outcome;`,
            PAGE_LIBRARY_NAME,
            id,
          ),
        { what: `${name} to finish in the page` },
      );
    }
    if ("error" in outcome) throw new Error(`${name} failed in the page: ${outcome.error}`);
    return (outcome as { value: unknown }).value as Awaited<ReturnType<PageLibrary[K]>>;
  }

  /** A token no other output of this terminal contains. */
  token(prefix = "t"): string {
    const id = `${prefix}${String(this.nextId)}x${Math.random().toString(36).slice(2, 8)}`;
    this.nextId += 1;
    return id;
  }

  isReady(): Promise<boolean> {
    return this.call("isReady");
  }

  state(): Promise<TerminalState> {
    return this.call("state");
  }

  layout(): Promise<PageLayout> {
    return this.call("layout");
  }

  /** What xterm.js has drawn, counted against the background of the pane. */
  async drawing(): Promise<Drawing> {
    const { background } = await this.layout();
    return this.call("drawing", parseColour(background), CHANNEL_TOLERANCE);
  }

  /** The last lines of the terminal, read from the buffer of xterm.js. */
  lines(tail = DEFAULT_TAIL): Promise<string[]> {
    return this.call("lines", tail);
  }

  /** Delivers key events to the terminal. See `keys.ts`. */
  keys(events: KeyEvent[]): Promise<void> {
    return this.call("keys", events);
  }

  /** Types printable ASCII text, one key at a time. */
  type(text: string): Promise<void> {
    return this.keys(typing(text));
  }

  /** Types a command line and presses Enter. */
  submit(commandLine: string): Promise<void> {
    return this.keys([...typing(commandLine), ...enter()]);
  }

  /** Presses Ctrl+C. */
  interrupt(): Promise<void> {
    return this.keys(control("c"));
  }

  /** Pastes text, as the webview does for Cmd+V or its menus. */
  async paste(text: string): Promise<void> {
    const taken = await this.call("paste", text);
    if (!taken) throw new Error("the application did not take the paste event");
  }

  throwUncaught(message: string): Promise<void> {
    return this.call("throwUncaught", message);
  }

  readClipboard(): Promise<string> {
    return this.call("readClipboard");
  }

  /** Round trip to the event loop of the page, in milliseconds. */
  async ping(): Promise<number> {
    const start = performance.now();
    await this.call("ping");
    return performance.now() - start;
  }

  /** Waits until `condition` holds for the state of the terminal. */
  waitForState(
    what: string,
    condition: (state: TerminalState) => boolean,
    timeoutMs = DEFAULT_TIMEOUT_MS,
  ): Promise<TerminalState> {
    let last: TerminalState | undefined;
    return waitFor(
      async () => {
        last = await this.state();
        return condition(last) ? last : undefined;
      },
      { what, timeoutMs, describe: () => last },
    );
  }

  /**
   * Waits until one of the last lines of the terminal satisfies `match`,
   * and returns what `match` returned for it.
   */
  waitForLine<T>(
    what: string,
    match: (line: string) => Awaitable<T | undefined | null | false>,
    options: { timeoutMs?: number; tail?: number } = {},
  ): Promise<T> {
    let last: string[] = [];
    return waitFor(
      async () => {
        last = await this.lines(options.tail ?? DEFAULT_TAIL);
        for (const line of last) {
          const matched = await match(line);
          if (matched !== undefined && matched !== null && matched !== false) return matched;
        }
        return undefined;
      },
      {
        what,
        timeoutMs: options.timeoutMs,
        describe: () => `The last lines of the terminal:\n${last.slice(-40).join("\n")}`,
      },
    );
  }

  /** Waits for a line that is exactly `text`. */
  async waitForExactLine(text: string, timeoutMs?: number): Promise<void> {
    await this.waitForLine(`the line ${JSON.stringify(text)}`, (line) => line === text, {
      timeoutMs,
    });
  }

  /**
   * Runs `echo` with a fresh token and waits for its output, which shows
   * that the shell is at its prompt and has run everything typed before.
   */
  async waitForPrompt(timeoutMs?: number): Promise<void> {
    const token = this.token("ready");
    await this.submit(`echo ${token}`);
    await this.waitForExactLine(token, timeoutMs);
  }

  /** Starts a program of `helper.mjs` in the terminal. */
  async startHelper(command: string, ...args: string[]): Promise<HelperRun> {
    const id = this.token("h");
    const commandLine = ["node", `"${HELPER_PATH}"`, command, id, ...args].join(" ");
    await this.submit(commandLine);
    return { id };
  }

  /**
   * Starts the program that prints the bytes it receives, and waits until
   * it reads its input.
   */
  async startRaw(options: { bracketedPaste?: boolean } = {}): Promise<RawInput> {
    const flags = options.bracketedPaste ? ["--bracketed"] : [];
    const { id } = await this.startHelper("raw", ...flags);
    await this.waitForExactLine(`RAW-READY-${id}`);
    if (options.bracketedPaste) {
      await this.waitForState("bracketed paste to be on", (state) => state.bracketedPaste);
    }
    return new RawInput(this, id);
  }
}

/** The program that prints, in hexadecimal, every byte the terminal sends it. */
export class RawInput {
  constructor(
    private readonly terminal: Terminal,
    readonly id: string,
  ) {}

  /**
   * Waits until what the program has received so far is exactly these
   * bytes: a byte too many, before or after, never matches.
   */
  async waitForBytes(hex: string, timeoutMs?: number): Promise<void> {
    const prefix = `RAW-${this.id}:`;
    const expected = `${prefix}${hex.replaceAll(" ", "")}:END`;
    let last: string[] = [];
    await waitFor(
      async () => {
        last = (await this.terminal.lines()).filter((line) => line.startsWith(prefix));
        return last.at(-1) === expected;
      },
      {
        what: `the program to have received ${expected}`,
        timeoutMs,
        describe: () => `What it printed:\n${last.join("\n")}`,
      },
    );
  }

  /** Ends the program with Ctrl+C and waits for the shell to be back. */
  async stop(): Promise<void> {
    await this.terminal.interrupt();
    await this.terminal.waitForExactLine(`RAW-EXIT-${this.id}`);
  }
}

/** The bytes of `text` in UTF-8, in hexadecimal. */
export function hexOf(text: string): string {
  return Buffer.from(text, "utf8").toString("hex");
}
