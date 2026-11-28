/**
 * A minimal client for the W3C WebDriver protocol: the few commands the
 * suite needs, over `fetch`, without a client library.
 *
 * It talks to the WebDriver server that runs inside builds of the
 * application made for this suite (see `apps/desktop/src-tauri/src/e2e.rs`).
 */

import { waitFor } from "./wait.ts";

/** How long one command may take, in milliseconds. */
const COMMAND_TIMEOUT_MS = 60_000;

/**
 * How long a script may run in the page, in milliseconds. The scripts of
 * the suite return at once: one that does not was sent to a page that is
 * not there.
 */
const SCRIPT_TIMEOUT_MS = 15_000;

/** How long the script that asks whether the page is loaded may run. */
const PAGE_PROBE_TIMEOUT_MS = 1_000;

/**
 * Commands that take longer than this, in milliseconds, are reported. A
 * healthy one answers in a few milliseconds; one that waited for a timeout
 * explains a step that took much longer than usual.
 */
const SLOW_COMMAND_MS = 2_000;

/** Reports a slow command: what it was, how long it took, how it ended. */
function reportSlow(method: string, path: string, started: number, outcome: string): void {
  const elapsed = Math.round(performance.now() - started);
  if (elapsed < SLOW_COMMAND_MS) return;
  // The session in the path is noise; the command is what matters.
  const command = path.replace(/^\/session\/[^/]+/, "");
  console.warn(
    `[webdriver] slow command: ${method} ${command || "/"} ${String(elapsed)} ms, ${outcome}`,
  );
}

export class WebDriverError extends Error {
  constructor(
    message: string,
    /** The error code of the protocol, such as `javascript error`. */
    readonly code: string,
  ) {
    super(message);
    this.name = "WebDriverError";
  }
}

interface Envelope {
  value: unknown;
}

function isEnvelope(body: unknown): body is Envelope {
  return typeof body === "object" && body !== null && "value" in body;
}

function errorOf(value: unknown): { error: string; message: string } | undefined {
  if (typeof value !== "object" || value === null) return undefined;
  const { error, message } = value as { error?: unknown; message?: unknown };
  if (typeof error !== "string") return undefined;
  return { error, message: typeof message === "string" ? message : "" };
}

async function request(
  origin: string,
  method: "GET" | "POST" | "DELETE",
  path: string,
  body?: unknown,
): Promise<unknown> {
  const started = performance.now();
  let response: Response;
  let text: string;
  try {
    response = await fetch(`${origin}${path}`, {
      method,
      headers: body === undefined ? undefined : { "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(COMMAND_TIMEOUT_MS),
    });
    text = await response.text();
  } catch (error) {
    reportSlow(method, path, started, `no answer (${String(error)})`);
    throw error;
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    throw new WebDriverError(
      `${method} ${path} answered ${String(response.status)} without JSON: ${text.slice(0, 200)}`,
      "unknown error",
    );
  }
  const value = isEnvelope(parsed) ? parsed.value : parsed;
  const failure = errorOf(value);
  reportSlow(method, path, started, failure?.error ?? `status ${String(response.status)}`);
  if (failure || !response.ok) {
    throw new WebDriverError(
      `${method} ${path} failed: ${failure?.message ?? text.slice(0, 200)}`,
      failure?.error ?? "unknown error",
    );
  }
  return value;
}

/** Whether the server at `origin` answers and has a window to drive. */
export async function isReady(origin: string): Promise<boolean> {
  try {
    const status = (await request(origin, "GET", "/status")) as { ready?: unknown };
    return status.ready === true;
  } catch {
    return false;
  }
}

/** An open WebDriver session with the application. */
export class WebDriver {
  private constructor(
    private readonly origin: string,
    private readonly sessionId: string,
  ) {}

  static async connect(origin: string): Promise<WebDriver> {
    const created = (await request(origin, "POST", "/session", {
      capabilities: { alwaysMatch: {} },
    })) as { sessionId?: unknown };
    if (typeof created.sessionId !== "string") {
      throw new WebDriverError("the server did not create a session", "session not created");
    }
    return new WebDriver(origin, created.sessionId);
  }

  private command(method: "GET" | "POST" | "DELETE", path: string, body?: unknown) {
    return request(this.origin, method, `/session/${this.sessionId}${path}`, body);
  }

  /** Runs `script` in the page. Its `return` value comes back. */
  async execute<T>(script: string, ...args: unknown[]): Promise<T> {
    return (await this.command("POST", "/execute/sync", { script, args })) as T;
  }

  /**
   * Waits until the page of the application has finished loading. The
   * server answers before the page is there, and a script sent while the
   * webview is still loading is never answered, or runs in a document that
   * is about to be replaced.
   */
  async waitForPage(timeoutMs: number): Promise<void> {
    // A script that is not answered is given up quickly, and sent again.
    await this.command("POST", "/timeouts", { script: PAGE_PROBE_TIMEOUT_MS });
    await waitFor(
      () =>
        this.execute<boolean>(
          `return document.readyState === "complete" && location.protocol !== "about:";`,
        ).catch(() => false),
      { what: "the page of the application to be loaded", timeoutMs },
    );
    await this.command("POST", "/timeouts", { script: SCRIPT_TIMEOUT_MS });
  }

  /** A screenshot of the page, as a PNG file. */
  async screenshot(): Promise<Buffer> {
    const encoded = (await this.command("GET", "/screenshot")) as string;
    return Buffer.from(encoded, "base64");
  }

  /** Asks for a window of this size, in the units of the windowing system. */
  async setWindowSize(width: number, height: number): Promise<void> {
    await this.command("POST", "/window/rect", { width, height });
  }

  /**
   * Closes the window, as its close button does. The application quits
   * with its last window, so the answer may never arrive.
   */
  async closeWindow(): Promise<void> {
    await this.command("DELETE", "/window");
  }
}
