import { type ChildProcess, spawn } from "node:child_process";
import {
  appendFileSync,
  closeSync,
  existsSync,
  mkdirSync,
  openSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { mkdtemp, readdir, readFile } from "node:fs/promises";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { delimiter, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { Terminal } from "./terminal.ts";
import { delay, waitFor } from "./wait.ts";
import { isReady, WebDriver } from "./webdriver.ts";

const REPOSITORY_ROOT = fileURLToPath(new URL("../..", import.meta.url));

/** Where everything worth keeping after a failure is written. */
export const ARTIFACTS_DIR = fileURLToPath(new URL("../artifacts", import.meta.url));

/**
 * One line per start of the application that had to be repeated. The suite
 * never repeats a test; this file is how repeated starts stay visible.
 * `run.mjs` empties it when a run of the suite begins.
 */
export const STARTUP_RETRIES_FILE = join(ARTIFACTS_DIR, "startup-retries.log");

/** How long the application may take to start and show its window. */
const STARTUP_TIMEOUT_MS = 90_000;

/**
 * How many starts may be repeated in one run of the suite, over all its
 * files. A start is only repeated for a race on the port of the WebDriver
 * server (see `App.start`), which the suite cannot rule out: it picks a free
 * port, and something else can take it before the application does. That
 * happening once in a run is bad luck; twice is something to look at, so
 * the second fails the test instead.
 */
const MAX_REPEATED_STARTS_PER_RUN = 1;

/**
 * What the WebDriver server logs when its port was taken
 * (`tauri-plugin-wdio-webdriver`, `server/mod.rs`). The application keeps
 * running without a server.
 */
const PORT_TAKEN_LOG = "Failed to bind WebDriver server";

/** The identifier of the application, in `tauri.conf.json`. */
export const APP_IDENTIFIER = "io.github.jibaru.termx";

/** How long the application may take to exit once it was asked to. */
export const EXIT_TIMEOUT_MS = 30_000;

/**
 * The shell the terminals run: one that reads no profile and no rc file,
 * with a fixed prompt, so that the tests do not depend on the shell or the
 * dotfiles of whoever runs them.
 */
export const SHELL: { command: string[]; env: Record<string, string> } =
  process.platform === "win32"
    ? {
        // /D: no AutoRun commands from the registry.
        command: [process.env.ComSpec ?? "cmd.exe", "/D"],
        env: { PROMPT: "termx-e2e$G " },
      }
    : {
        command: ["/bin/bash", "--noprofile", "--norc"],
        env: { PS1: "termx-e2e$ ", BASH_SILENCE_DEPRECATION_WARNING: "1" },
      };

/** The application binary built for the suite: `pnpm e2e:build`. */
export function binaryPath(): string {
  const name = process.platform === "win32" ? "termx-desktop.exe" : "termx-desktop";
  return process.env.TERMX_E2E_BINARY ?? join(REPOSITORY_ROOT, "target", "debug", name);
}

/** A port nothing listens on right now. */
function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close(() => {
        if (address !== null && typeof address === "object") resolve(address.port);
        else reject(new Error("no port could be reserved"));
      });
    });
  });
}

export interface LaunchOptions {
  /**
   * The home directory of an earlier run, to start termx again on what it
   * saved there (its state directory is inside). Made with `createHome`,
   * and removed with `removeHome` by whoever passes it, not by the `App`.
   */
  home?: string;
  /**
   * The terminal on screen is a session restored from an earlier run: wait
   * for it to be dormant, waiting for Enter, rather than for a shell at its
   * prompt.
   */
  restored?: boolean;
  /**
   * Windows only: start the application the way a process that ignores
   * Ctrl+C does, which the application inherits.
   */
  ignoringCtrlC?: boolean;
}

export interface ExitInfo {
  code: number | null;
  signal: NodeJS.Signals | null;
}

/**
 * Where WebView2 keeps its profile (cookies, storage, caches) in a run on
 * Windows, inside `home`.
 *
 * Tauri passes WebView2 `<local app data>/<identifier>`, where it resolves
 * the local app data folder with `SHGetKnownFolderPath`, not from
 * `LOCALAPPDATA`. WebView2 gives `WEBVIEW2_USER_DATA_FOLDER` precedence over
 * that, so the suite sets it. `specs/launch.e2e.ts` checks the result.
 */
export function webviewDataDir(home: string): string {
  return join(home, "WebView2");
}

/**
 * The environment of the application: the one of the suite, with every
 * directory an application or a shell keeps state in moved to `home`, and
 * none of the settings of termx (`TERMX_*`) of whoever runs the suite.
 */
function environment(home: string, logDir: string, port: number): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { ...process.env, ...SHELL.env };
  for (const name of Object.keys(env)) {
    const upper = name.toUpperCase();
    if (
      upper.startsWith("TERMX_") ||
      upper.startsWith("WEBVIEW2_") ||
      ["LC_ALL", "LC_CTYPE", "ENV", "BASH_ENV"].includes(upper)
    ) {
      Reflect.deleteProperty(env, name);
    }
  }
  const state = {
    HOME: home,
    XDG_CONFIG_HOME: join(home, ".config"),
    XDG_DATA_HOME: join(home, ".local", "share"),
    XDG_STATE_HOME: join(home, ".local", "state"),
    XDG_CACHE_HOME: join(home, ".cache"),
    LANG: process.platform === "darwin" ? "en_US.UTF-8" : "C.UTF-8",
  };
  const windows = {
    USERPROFILE: home,
    APPDATA: join(home, "AppData", "Roaming"),
    LOCALAPPDATA: join(home, "AppData", "Local"),
    WEBVIEW2_USER_DATA_FOLDER: webviewDataDir(home),
  };
  return {
    ...env,
    ...(process.platform === "win32" ? windows : state),
    // The tests run `node` in the terminal: the one that runs the suite.
    PATH: `${dirname(process.execPath)}${delimiter}${process.env.PATH ?? ""}`,
    // Every directory termx keeps something in, set explicitly rather than
    // derived from the home directory.
    TERMX_STATE_DIR: join(home, "termx-state"),
    TERMX_LOG_DIR: logDir,
    TERMX_LOG: "debug",
    TERMX_E2E_SHELL: SHELL.command.join("\n"),
    TAURI_WEBDRIVER_PORT: String(port),
  };
}

/** A running instance of the application, driven through WebDriver. */
export class App {
  private exit: ExitInfo | undefined;

  private constructor(
    /** Name of the run, which is also its directory under `artifacts/`. */
    readonly name: string,
    private readonly child: ChildProcess,
    readonly driver: WebDriver,
    readonly terminal: Terminal,
    /** The home directory the application and its shells run with. */
    readonly home: string,
    /** Whether the home directory goes with this run (see `LaunchOptions.home`). */
    private readonly ownsHome: boolean,
    /** Where the application writes its log files. */
    readonly logDir: string,
  ) {
    child.once("exit", (code, signal) => {
      this.exit = { code, signal };
    });
  }

  /** The process identifier of the application. */
  get pid(): number {
    if (this.child.pid === undefined) throw new Error("the application has no process");
    return this.child.pid;
  }

  /**
   * Starts the application and waits until its terminal runs a shell that
   * is at its prompt.
   *
   * A start is repeated only when the WebDriver server of the application
   * could not listen on the port the suite picked, a race the suite cannot
   * rule out, and at most `MAX_REPEATED_STARTS_PER_RUN` times in a run of
   * the suite. Every repeated start is recorded in `STARTUP_RETRIES_FILE`.
   * Anything else fails the test: an application that exits while it
   * starts, with what it printed and logged; a server that never answers;
   * and everything after a WebDriver session exists (a page that does not
   * load, a window that stays empty, a shell that does not start), with a
   * description of the page (`captureStartupDiagnostics`).
   */
  static async launch(name: string, options: LaunchOptions = {}): Promise<App> {
    const binary = binaryPath();
    if (!existsSync(binary)) {
      throw new Error(
        `${binary} does not exist. Build the application for the suite with "pnpm e2e:build".`,
      );
    }
    for (let attempt = 1; ; attempt += 1) {
      const runName = attempt === 1 ? name : `${name}-attempt-${String(attempt)}`;
      let app: App;
      try {
        app = await App.start(binary, runName, options);
      } catch (error) {
        if (!(error instanceof PortRace)) throw error;
        const repeated = repeatedStarts();
        const message = `${new Date().toISOString()} ${name}: start ${String(attempt)} failed: ${error.message}`;
        mkdirSync(ARTIFACTS_DIR, { recursive: true });
        appendFileSync(STARTUP_RETRIES_FILE, `${message}\n`);
        if (repeated >= MAX_REPEATED_STARTS_PER_RUN) {
          throw new Error(
            `${message}. ${String(repeated)} start(s) were already repeated in this run of the suite, ` +
              `and at most ${String(MAX_REPEATED_STARTS_PER_RUN)} may be: see ${STARTUP_RETRIES_FILE}`,
            { cause: error },
          );
        }
        console.warn(`${message}; starting it again`);
        continue;
      }
      // From here on a failure is a failure of the application.
      try {
        await app.driver.waitForPage(STARTUP_TIMEOUT_MS);
        await waitFor(() => app.terminal.isReady(), {
          what: "the application to put a terminal on screen",
          timeoutMs: STARTUP_TIMEOUT_MS,
        });
        if (options.restored === true) {
          await app.terminal.waitForState(
            "the restored session to wait for Enter",
            (state) => state.status.kind === "dormant",
          );
          return app;
        }
        await app.terminal.waitForState(
          "the shell to be running",
          (state) => state.status.kind === "running",
        );
        await app.terminal.waitForPrompt();
        return app;
      } catch (error) {
        const diagnostics = await app.captureStartupDiagnostics();
        await app.dispose();
        throw new Error(`${String(error)}\n\n${diagnostics}`, { cause: error });
      }
    }
  }

  private static async start(binary: string, name: string, options: LaunchOptions): Promise<App> {
    const runDir = join(ARTIFACTS_DIR, name);
    rmSync(runDir, { recursive: true, force: true });
    const logDir = join(runDir, "logs");
    mkdirSync(logDir, { recursive: true });
    const ownsHome = options.home === undefined;
    const home = options.home ?? (await createHome());
    const port = await freePort();

    // Everything the application prints, which in a debug build includes
    // its log records.
    const outputPath = join(runDir, "app-output.log");
    const output = openSync(outputPath, "w");
    const child = spawn(binary, [], {
      cwd: home,
      env: environment(home, logDir, port),
      stdio: ["ignore", output, output],
      // On Windows a detached process is the leader of a new process
      // group, which is started with Ctrl+C ignored.
      detached: options.ignoringCtrlC === true && process.platform === "win32",
    });
    closeSync(output);

    let exited: ExitInfo | undefined;
    let spawnError: Error | undefined;
    child.once("exit", (code, signal) => {
      exited = { code, signal };
    });
    child.once("error", (error) => {
      spawnError = error;
    });

    const origin = `http://127.0.0.1:${String(port)}`;
    try {
      await waitFor(
        async () => {
          if (spawnError) throw spawnError;
          if (exited) {
            throw new Error(
              `the application exited while starting (${JSON.stringify(exited)})\n\n` +
                outputOfRun(outputPath, logDir),
            );
          }
          if (readText(outputPath).includes(PORT_TAKEN_LOG)) {
            throw new PortRace(`its WebDriver server could not listen on port ${String(port)}`);
          }
          return isReady(origin);
        },
        {
          what: "the WebDriver server of the application",
          timeoutMs: STARTUP_TIMEOUT_MS,
          describe: () => outputOfRun(outputPath, logDir),
        },
      );
      let driver: WebDriver;
      try {
        driver = await WebDriver.connect(origin);
      } catch (error) {
        // `fetch` rejects with a TypeError when the connection itself
        // fails: the server that answered is gone, or was not ours.
        if (error instanceof TypeError && !exited) {
          throw new PortRace(`its WebDriver server stopped answering: ${String(error.cause)}`, {
            cause: error,
          });
        }
        throw error;
      }
      return new App(name, child, driver, new Terminal(driver), home, ownsHome, logDir);
    } catch (error) {
      child.kill("SIGKILL");
      await waitFor(() => exited ?? spawnError, {
        what: "the application to exit after it was killed",
        timeoutMs: EXIT_TIMEOUT_MS,
      }).catch(() => undefined);
      if (ownsHome) await removeHome(home);
      throw error;
    }
  }

  /**
   * Describes the page and the terminal, when the application started but
   * its terminal did not get to a prompt: what helps to tell a page that
   * never ran, a window the system throttles (hidden, unfocused), a session
   * whose output never reached xterm.js, and a shell that did not start.
   * Writes it next to the logs of the run and returns it, with the last
   * lines of the log, to be attached to the failure.
   */
  async captureStartupDiagnostics(): Promise<string> {
    const answer = await Promise.race([
      this.terminal.diagnostics().catch((error: unknown) => ({ unavailable: String(error) })),
      delay(20_000).then(() => ({ unavailable: "the page did not answer within 20 s" })),
    ]);
    const diagnostics = { process: this.exitInfo() ?? "running", page: answer };
    const path = join(this.artifactsDir, "startup-diagnostics.json");
    try {
      writeFileSync(path, JSON.stringify(diagnostics, null, 2));
    } catch {
      // The description is in the message of the failure as well.
    }
    return [
      `Startup diagnostics (also in ${path}):`,
      JSON.stringify(diagnostics, null, 2),
      outputOfRun(join(this.artifactsDir, "app-output.log"), this.logDir),
    ].join("\n");
  }

  /** The directory of this run under `artifacts/`. */
  get artifactsDir(): string {
    return join(ARTIFACTS_DIR, this.name);
  }

  /** Whether the process of the application has exited, and how. */
  exitInfo(): ExitInfo | undefined {
    return this.exit;
  }

  /**
   * Closes the window, which is how a person quits: the application exits
   * with its last window. While sessions run, termx asks first, and the
   * answer is yes, as a person who closes the window means it. Returns how
   * the process ended.
   */
  async quit(): Promise<ExitInfo> {
    // The answer is lost when the server goes away with the application.
    await this.driver.closeWindow().catch(() => undefined);
    return waitFor(
      async () => {
        if (this.exit) return this.exit;
        await this.confirmQuit();
        return undefined;
      },
      {
        what: "the application to exit after its window was closed",
        timeoutMs: EXIT_TIMEOUT_MS,
      },
    );
  }

  /** Says yes to the confirmation before quitting, if it is shown. */
  private async confirmQuit(): Promise<void> {
    await this.driver
      .execute(
        `const button = document.querySelector('[data-testid="confirm-quit"]');
         if (button) button.click();
         return button !== null;`,
      )
      .catch(() => undefined);
  }

  /** Ends the application whatever its state, and removes its home if it made it. */
  async dispose(): Promise<void> {
    if (!this.exitInfo()) {
      await this.driver.closeWindow().catch(() => undefined);
      const deadline = Date.now() + 5_000;
      while (!this.exitInfo() && Date.now() < deadline) {
        await this.confirmQuit();
        await delay(50);
      }
      if (!this.exitInfo()) this.child.kill("SIGKILL");
    }
    if (this.ownsHome) await removeHome(this.home);
  }

  /** The text of every log file the application has written. */
  async readLogs(): Promise<string> {
    const names = (await readdir(this.logDir)).filter((name) => name.endsWith(".log")).sort();
    const contents = await Promise.all(
      names.map((name) => readFile(join(this.logDir, name), "utf8")),
    );
    return contents.join("\n");
  }
}

/**
 * A start that lost a race for the port of the WebDriver server: the only
 * kind of start that is repeated.
 */
class PortRace extends Error {
  constructor(message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = "PortRace";
  }
}

/** How many starts were repeated so far in this run of the suite. */
function repeatedStarts(): number {
  return readText(STARTUP_RETRIES_FILE)
    .split("\n")
    .filter((line) => line !== "").length;
}

function readText(path: string): string {
  try {
    return readFileSync(path, "utf8");
  } catch {
    return "";
  }
}

/** The last lines of a text, to quote in a failure. */
function tail(text: string, lines: number): string {
  return text.split("\n").slice(-lines).join("\n");
}

/** What the application printed and logged, to attach to a failure. */
function outputOfRun(outputPath: string, logDir: string): string {
  return [
    `The last lines the application printed (${outputPath}):`,
    tail(readText(outputPath), 60) || "(nothing)",
    `Its log files are in ${logDir}.`,
  ].join("\n");
}

/** A new, empty home directory for a run of the application. */
export function createHome(): Promise<string> {
  return mkdtemp(join(tmpdir(), "termx-e2e-home-"));
}

/**
 * Removes a home directory of a run. The processes of WebView2 can keep
 * files open, and even write new ones, for a few seconds after the
 * application is gone, so this tries for a while, and only warns if it
 * cannot.
 */
export async function removeHome(home: string): Promise<void> {
  const deadline = Date.now() + 30_000;
  for (;;) {
    try {
      rmSync(home, { recursive: true, force: true, maxRetries: 3, retryDelay: 200 });
      return;
    } catch (error) {
      if (Date.now() >= deadline) {
        console.warn(`the home directory of a run could not be removed: ${String(error)}`);
        return;
      }
      await delay(500);
    }
  }
}
