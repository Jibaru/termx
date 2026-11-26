import { invoke, type InvokeArgs } from "@tauri-apps/api/core";

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

/** Records sent to the log per window of `LOG_WINDOW_MS`, at most. */
export const LOG_LIMIT = 20;
export const LOG_WINDOW_MS = 10_000;

let logWindowStart = 0;
let logWindowCount = 0;

/**
 * Writes a record to the log file. This is the only way the frontend logs.
 *
 * It never throws and never rejects, because a logging problem must not
 * cause another error. Records over the limit are dropped here, before they
 * reach the Rust side, which applies its own rate limit as well.
 *
 * Privacy: never pass terminal output, typed input, environment variable
 * values or file contents. See docs/logging.md.
 */
export async function logFrontendEvent(event: FrontendEvent): Promise<void> {
  const now = Date.now();
  if (now - logWindowStart >= LOG_WINDOW_MS) {
    logWindowStart = now;
    logWindowCount = 0;
  }
  if (logWindowCount >= LOG_LIMIT) return;
  logWindowCount += 1;

  try {
    await invoke(LOG_COMMAND, { event });
  } catch {
    // Nowhere left to report to.
  }
}

/** Forgets the records counted so far. For tests. */
export function resetFrontendLogLimit(): void {
  logWindowStart = 0;
  logWindowCount = 0;
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

/** Whether a rejected call carries the `CommandError` of a Rust command. */
export function isCommandError(error: unknown): error is CommandError {
  return (
    typeof error === "object" &&
    error !== null &&
    typeof (error as Partial<CommandError>).command === "string" &&
    typeof (error as Partial<CommandError>).message === "string"
  );
}

/** A readable description of whatever a call rejected or a `throw` threw. */
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
        message: describeError(error),
        stack: error instanceof Error ? error.stack : undefined,
      });
    }
    throw error;
  }
}
