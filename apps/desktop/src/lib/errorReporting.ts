import { describeError, logFrontendEvent } from "./ipc";

/**
 * Sends frontend errors to the log file kept by the Rust side.
 *
 * Everything goes through `logFrontendEvent`, which is rate limited and
 * cannot fail.
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
    message: describeError(error),
    stack: error instanceof Error ? error.stack : undefined,
    componentStack: extra.componentStack,
  });
}

/**
 * Records uncaught errors and unhandled promise rejections of `target`.
 * Returns a function that removes the handlers again.
 */
export function installGlobalErrorHandlers(target: Window = window): () => void {
  const onError = (event: ErrorEvent) => {
    // `event.error` is missing for errors in cross-origin scripts.
    const error: unknown = event.error ?? event.message;
    void logFrontendEvent({
      level: "error",
      source: "window.onerror",
      message: describeError(error),
      stack:
        error instanceof Error
          ? error.stack
          : `${event.filename}:${String(event.lineno)}:${String(event.colno)}`,
    });
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
