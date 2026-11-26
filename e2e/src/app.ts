import { type ChildProcess, spawn } from "node:child_process";
import { appendFileSync, closeSync, existsSync, mkdirSync, openSync, rmSync } from "node:fs";
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
 */
export const STARTUP_RETRIES_FILE = join(ARTIFACTS_DIR, "startup-retries.log");

/** How long the application may take to start and show its window. */
const STARTUP_TIMEOUT_MS = 90_000;

/** How often the application is started before a test is failed for it. */
const STARTUP_ATTEMPTS = 3;

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
 * The environment of the application: the one of the suite, with every
 * directory an application or a shell keeps state in moved to `home`.
 */
function environment(home: string, logDir: string, port: number): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { ...process.env, ...SHELL.env };
  for (const name of ["LC_ALL", "LC_CTYPE", "ENV", "BASH_ENV", "TERMX_LOG_DIR", "TERMX_LOG"]) {
    Reflect.deleteProperty(env, name);
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
  };
  return {
    ...env,
    ...(process.platform === "win32" ? windows : state),
    // The tests run `node` in the terminal: the one that runs the suite.
    PATH: `${dirname(process.execPath)}${delimiter}${process.env.PATH ?? ""}`,
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
   * A start that does not get as far as a WebDriver session (the process
   * does not start, or its WebDriver server does not answer) is repeated,
   * and recorded in `STARTUP_RETRIES_FILE`. Nothing after that point is
   * ever repeated: a page that does not load, a window that stays empty or
   * a shell that does not start fails the test.
   */
  static async launch(name: string, options: LaunchOptions = {}): Promise<App> {
    const binary = binaryPath();
    if (!existsSync(binary)) {
      throw new Error(
        `${binary} does not exist. Build the application for the suite with "pnpm e2e:build".`,
      );
    }
    let failure: unknown;
    for (let attempt = 1; attempt <= STARTUP_ATTEMPTS; attempt += 1) {
      const runName = attempt === 1 ? name : `${name}-attempt-${String(attempt)}`;
      let app: App;
      try {
        app = await App.start(binary, runName, options);
      } catch (error) {
        if (!(error instanceof StartupError)) throw error;
        failure = error.cause;
        const message = `${new Date().toISOString()} ${name}: start ${String(attempt)} of ${String(STARTUP_ATTEMPTS)} failed: ${String(error.cause)}`;
        console.warn(message);
        mkdirSync(ARTIFACTS_DIR, { recursive: true });
        appendFileSync(STARTUP_RETRIES_FILE, `${message}\n`);
        continue;
      }
      // From here on a failure is a failure of the application.
      try {
        await app.driver.waitForPage(STARTUP_TIMEOUT_MS);
        await waitFor(() => app.terminal.isReady(), {
          what: "the application to put a terminal on screen",
          timeoutMs: STARTUP_TIMEOUT_MS,
        });
        await app.terminal.waitForState(
          "the shell to be running",
          (state) => state.status.kind === "running",
        );
        await app.terminal.waitForPrompt();
        return app;
      } catch (error) {
        await app.dispose();
        throw error;
      }
    }
    throw new Error(`the application could not be started: ${String(failure)}`);
  }

  private static async start(binary: string, name: string, options: LaunchOptions): Promise<App> {
    const runDir = join(ARTIFACTS_DIR, name);
    rmSync(runDir, { recursive: true, force: true });
    const logDir = join(runDir, "logs");
    mkdirSync(logDir, { recursive: true });
    const home = await mkdtemp(join(tmpdir(), "termx-e2e-home-"));
    const port = await freePort();

    // Everything the application prints, which in a debug build includes
    // its log records.
    const output = openSync(join(runDir, "app-output.log"), "w");
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
            throw new Error(`the application exited while starting: ${JSON.stringify(exited)}`);
          }
          return isReady(origin);
        },
        { what: "the WebDriver server of the application", timeoutMs: STARTUP_TIMEOUT_MS },
      );
      const driver = await WebDriver.connect(origin);
      return new App(name, child, driver, new Terminal(driver), home, logDir);
    } catch (error) {
      child.kill("SIGKILL");
      throw new StartupError(error);
    }
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
   * with its last window. Returns how the process ended.
   */
  async quit(): Promise<ExitInfo> {
    // The answer is lost when the server goes away with the application.
    await this.driver.closeWindow().catch(() => undefined);
    return waitFor(() => this.exit, {
      what: "the application to exit after its window was closed",
      timeoutMs: EXIT_TIMEOUT_MS,
    });
  }

  /** Ends the application whatever its state, and removes its home. */
  async dispose(): Promise<void> {
    if (!this.exitInfo()) {
      await this.driver.closeWindow().catch(() => undefined);
      const deadline = Date.now() + 5_000;
      while (!this.exitInfo() && Date.now() < deadline) await delay(50);
      if (!this.exitInfo()) this.child.kill("SIGKILL");
    }
    rmSync(this.home, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
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

/** A start of the application that did not reach a WebDriver session. */
class StartupError extends Error {
  constructor(override readonly cause: unknown) {
    super("the application did not start");
    this.name = "StartupError";
  }
}
