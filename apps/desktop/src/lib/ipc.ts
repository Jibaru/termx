import { Channel, invoke, type InvokeArgs } from "@tauri-apps/api/core";
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";
import { openUrl } from "@tauri-apps/plugin-opener";

import { LogLimiter, type LogLimits } from "./logLimiter";
import { describeIpcFailure } from "./logSafe";

/**
 * Typed wrappers around the Tauri commands exposed by the Rust side
 * (apps/desktop/src-tauri/src/commands). Components call these instead of
 * using `invoke` directly, so command names live in a single place.
 *
 * Wrappers go through `call`, which records failed calls in the log.
 */

/** Returns the version of the `termx-core` library the app was built with. */
export function getCoreVersion(): Promise<string> {
  return call<string>("core_version");
}

/** Severity of a record sent to the log by the frontend. */
export type FrontendLogLevel = "error" | "warn" | "info";

/** A record for the log file. Mirrors `FrontendEvent` on the Rust side. */
export interface FrontendEvent {
  level: FrontendLogLevel;
  /** Where it was caught, such as `window.onerror`, `react` or `ipc`. */
  source: string;
  message: string;
  stack?: string;
  /** React component stack, for errors caught by an error boundary. */
  componentStack?: string;
  /** Name of the Tauri command, for failed IPC calls. */
  command?: string;
}

/** What a failed command rejects with. Mirrors `CommandError` in Rust. */
export interface CommandError {
  command: string;
  message: string;
}

/** Where a diagnostics bundle was written. */
export interface ExportedBundle {
  /** Full path of the zip file. */
  path: string;
}

const LOG_COMMAND = "log_frontend_event";

let logLimiter = new LogLimiter();

/**
 * Writes a record to the log file. This is the only way the frontend logs.
 *
 * It never throws and never rejects, because a logging problem must not
 * cause another error. Records over the limit are dropped here, before they
 * reach the Rust side, which applies its own rate limit as well. Drops are
 * counted, and the count is logged before the next record that gets through.
 *
 * Privacy: never pass terminal output, typed input, environment variable
 * values or file contents. See docs/logging.md.
 */
export async function logFrontendEvent(event: FrontendEvent): Promise<void> {
  const dropped = logLimiter.admit(event.level, `${event.source}\n${event.message}`);
  if (dropped === null) return;

  try {
    const total = dropped.error + dropped.warn + dropped.info;
    if (total > 0) {
      await invoke(LOG_COMMAND, {
        event: {
          level: "warn",
          source: "frontend-limit",
          message:
            `${String(total)} records were dropped by the frontend rate limit ` +
            `(${String(dropped.error)} errors, ${String(dropped.warn)} warnings, ` +
            `${String(dropped.info)} others)`,
        } satisfies FrontendEvent,
      });
    }
    await invoke(LOG_COMMAND, { event });
  } catch {
    // Nowhere left to report to.
  }
}

/** Starts the rate limit over, optionally with other limits. For tests. */
export function resetFrontendLogLimit(limits?: LogLimits, now?: () => number): void {
  logLimiter = new LogLimiter(limits, now);
}

/** Opens the folder that holds the log files in the file manager. */
export function openLogFolder(): Promise<void> {
  return call("open_log_folder");
}

/**
 * Writes a diagnostics bundle (a zip with the log files and
 * `system-info.json`) to the Downloads folder, opens that folder and returns
 * where the bundle is. The Rust side chooses the location; it cannot be set
 * from here.
 */
export function exportDiagnostics(): Promise<ExportedBundle> {
  return call<ExportedBundle>("export_diagnostics");
}

/** What a new terminal session runs. Mirrors `CreateSessionRequest` in Rust. */
export interface CreateSessionOptions {
  /** The program to run. The user's login shell when omitted. */
  program?: string;
  /** Arguments of `program`. */
  args?: string[];
  /** The working directory. The home directory when omitted. */
  cwd?: string;
  /** Width of the terminal, in columns. */
  cols: number;
  /** Height of the terminal, in rows. */
  rows: number;
}

/** A session that was started. Mirrors `CreatedSession` in Rust. */
export interface CreatedSession {
  /** Identifier to pass to the other session functions. */
  id: number;
  /**
   * How many processed bytes to accumulate before acknowledging them with
   * `ackSession`.
   */
  ackBatchBytes: number;
  /** The build number of Windows, or `null` on other systems. */
  windowsBuildNumber: number | null;
}

/** How the process of a session ended. */
export interface SessionExit {
  /** The exit code, or `null` if a signal ended the process. */
  code: number | null;
  /** Description of the signal that ended the process, if any. */
  signal: string | null;
}

/** Receives what a session produces. */
export interface SessionHandlers {
  /**
   * A batch of output, exactly as the process wrote it. Pass it to the
   * terminal emulator and acknowledge its length with `ackSession` once it
   * was processed. Never log it.
   */
  onOutput: (bytes: Uint8Array) => void;
  /** The process has exited. Nothing follows. */
  onExit: (exit: SessionExit) => void;
}

/** A JSON message of the session channel. Mirrors `SessionEvent` in Rust. */
type SessionEvent = { event: "exit" } & SessionExit;

/**
 * What arrives on the channel of a session: output as raw bytes, events as
 * JSON. The bytes are an `ArrayBuffer`, except through the fallback
 * transport Tauri uses when the webview cannot fetch from the custom
 * protocol, which delivers an array of numbers.
 */
type SessionMessage = ArrayBuffer | number[] | SessionEvent;

function isSessionEvent(message: SessionMessage): message is SessionEvent {
  return !Array.isArray(message) && "event" in message;
}

/**
 * Starts a terminal session and resolves with its identifier.
 *
 * Output can reach `handlers.onOutput`, and even the exit `handlers.onExit`,
 * before the promise resolves: a process may print, or end, at once. The
 * handlers are called in the order things happened.
 */
export function createSession(
  options: CreateSessionOptions,
  handlers: SessionHandlers,
): Promise<CreatedSession> {
  const onEvent = new Channel<SessionMessage>();
  onEvent.onmessage = (message) => {
    if (isSessionEvent(message)) {
      handlers.onExit({ code: message.code, signal: message.signal });
    } else {
      handlers.onOutput(
        Array.isArray(message) ? Uint8Array.from(message) : new Uint8Array(message),
      );
    }
  };
  return call<CreatedSession>("session_create", { request: options, onEvent });
}

/**
 * Sends input to a session, as if it had been typed.
 *
 * `data` is text. With `binary`, each of its characters stands for one byte
 * instead, which is how xterm.js reports input that is not text (`onBinary`).
 *
 * Wait for the promise before sending the next input of the same session:
 * that is what keeps input in order.
 */
export function writeSession(id: number, data: string, binary = false): Promise<void> {
  return call("session_write", { id, data, binary });
}

/** Changes the size of the terminal of a session. */
export function resizeSession(id: number, cols: number, rows: number): Promise<void> {
  return call("session_resize", { id, cols, rows });
}

/**
 * Tells the session that `bytes` of its output were processed. A session
 * whose output is not acknowledged is held back, so that a program printing
 * faster than the terminal can draw does not fill the memory.
 */
export function ackSession(id: number, bytes: number): Promise<void> {
  return call("session_ack", { id, bytes });
}

/** Closes a session, killing its process. */
export function closeSession(id: number): Promise<void> {
  return call("session_close", { id });
}

/**
 * Opens a link in the system browser. Only `http` and `https` links are
 * opened: the Rust side refuses everything else.
 */
export function openExternalUrl(url: string): Promise<void> {
  return openUrl(url);
}

/** The text on the system clipboard. */
export function readClipboardText(): Promise<string> {
  return readText();
}

/** Puts `text` on the system clipboard. */
export function writeClipboardText(text: string): Promise<void> {
  return writeText(text);
}

/** Whether a rejected call carries the `CommandError` of a Rust command. */
export function isCommandError(error: unknown): error is CommandError {
  return (
    typeof error === "object" &&
    error !== null &&
    typeof (error as Partial<CommandError>).command === "string" &&
    typeof (error as Partial<CommandError>).message === "string"
  );
}

/**
 * A readable description of whatever a call rejected or a `throw` threw, to
 * show in the interface. It may contain the thrown value itself, so it must
 * not be logged: use `describeForLog` from `logSafe.ts` for that.
 */
export function describeError(error: unknown): string {
  if (isCommandError(error)) return error.message;
  if (error instanceof Error) return error.message || error.name;
  if (typeof error === "string") return error;
  // `JSON.stringify` gives no string for undefined, functions and symbols,
  // and throws on cycles.
  if (typeof error !== "object" || error === null) return String(error);
  try {
    return JSON.stringify(error);
  } catch {
    return "error object that cannot be serialised";
  }
}

/**
 * Invokes a Tauri command and records the failure if it rejects.
 *
 * A `CommandError` was already logged by the Rust side, so only failures of
 * the call itself (unknown command, bad arguments, no permission) are logged
 * here. The rejection is passed on to the caller either way.
 */
async function call<T = void>(command: string, args?: InvokeArgs): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    if (!isCommandError(error)) {
      void logFrontendEvent({
        level: "error",
        source: "ipc",
        command,
        ...describeIpcFailure(error),
      });
    }
    throw error;
  }
}
