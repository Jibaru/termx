import { logFrontendEvent } from "./ipc";
import { describeForLog } from "./logSafe";

/**
 * Sends frontend errors to the log file kept by the Rust side.
 *
 * Everything goes through `logFrontendEvent`, which is rate limited and
 * cannot fail, and is described by `describeForLog`, which never logs the
 * value of something that is not an `Error`.
 */

/** Records an error caught anywhere in the frontend. */
export function reportError(
  source: string,
  error: unknown,
  extra: { componentStack?: string } = {},
): void {
  void logFrontendEvent({
    level: "error",
    source,
    ...describeForLog(error),
    componentStack: extra.componentStack,
  });
}

/**
 * Records uncaught errors and unhandled promise rejections of `target`.
 * Returns a function that removes the handlers again.
 */
export function installGlobalErrorHandlers(target: Window = window): () => void {
  const onError = (event: ErrorEvent) => {
    const thrown: unknown = event.error;
    // The browser gives no error object for errors in cross-origin scripts.
    // Its own message ("Script error.") and the location are all there is.
    const described =
      thrown === undefined || thrown === null
        ? {
            message: event.message,
            stack: `${event.filename}:${String(event.lineno)}:${String(event.colno)}`,
          }
        : describeForLog(thrown);
    void logFrontendEvent({ level: "error", source: "window.onerror", ...described });
  };
  const onRejection = (event: PromiseRejectionEvent) => {
    reportError("unhandledrejection", event.reason);
  };

  target.addEventListener("error", onError);
  target.addEventListener("unhandledrejection", onRejection);
  return () => {
    target.removeEventListener("error", onError);
    target.removeEventListener("unhandledrejection", onRejection);
  };
}
