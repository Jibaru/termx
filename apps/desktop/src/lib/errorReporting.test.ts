import { afterEach, describe, expect, it, vi } from "vitest";

import { recordIPC } from "../test/ipcRecorder";
import { installGlobalErrorHandlers, reportError } from "./errorReporting";

let uninstall: (() => void) | undefined;

afterEach(() => {
  uninstall?.();
  uninstall = undefined;
});

function rejectionEvent(reason: unknown): Event {
  return Object.assign(new Event("unhandledrejection"), { reason });
}

describe("installGlobalErrorHandlers", () => {
  it("logs uncaught errors with their stack", async () => {
    const ipc = recordIPC();
    uninstall = installGlobalErrorHandlers();
    const error = new Error("x is not a function");

    window.dispatchEvent(new ErrorEvent("error", { error, message: error.message }));

    await vi.waitFor(() => {
      expect(ipc.logged()).toHaveLength(1);
    });
    expect(ipc.logged()[0]).toMatchObject({
      level: "error",
      source: "window.onerror",
      message: "Error: x is not a function",
      stack: error.stack,
    });
  });

  it("logs uncaught errors that carry no error object with their location", async () => {
    const ipc = recordIPC();
    uninstall = installGlobalErrorHandlers();

    window.dispatchEvent(
      new ErrorEvent("error", {
        message: "Script error.",
        filename: "app.js",
        lineno: 12,
        colno: 3,
      }),
    );

    await vi.waitFor(() => {
      expect(ipc.logged()).toHaveLength(1);
    });
    expect(ipc.logged()[0]).toMatchObject({
      source: "window.onerror",
      message: "Script error.",
      stack: "app.js:12:3",
    });
  });

  it("logs unhandled promise rejections", async () => {
    const ipc = recordIPC();
    uninstall = installGlobalErrorHandlers();

    window.dispatchEvent(rejectionEvent(new Error("request failed")));
    window.dispatchEvent(rejectionEvent("rejected with a string"));

    await vi.waitFor(() => {
      expect(ipc.logged()).toHaveLength(2);
    });
    expect(ipc.logged()[0]).toMatchObject({
      level: "error",
      source: "unhandledrejection",
      message: "Error: request failed",
    });
    expect(ipc.logged()[1]).toMatchObject({
      source: "unhandledrejection",
      message: "non-Error value (string of length 22)",
    });
  });

  it("never logs the values of a rejection or an error that is not an Error", async () => {
    const ipc = recordIPC();
    uninstall = installGlobalErrorHandlers();
    const payload = { token: "hunter2-very-secret", env: { HOME: "/home/someone" } };

    window.dispatchEvent(rejectionEvent(payload));
    window.dispatchEvent(new ErrorEvent("error", { error: payload, message: "Uncaught hunter2" }));

    await vi.waitFor(() => {
      expect(ipc.logged()).toHaveLength(2);
    });
    expect(ipc.logged().map((event) => event.message)).toEqual([
      "non-Error value (object with 2 keys [token, env])",
      "non-Error value (object with 2 keys [token, env])",
    ]);
    expect(JSON.stringify(ipc.logged())).not.toMatch(/hunter2|someone/);
  });

  it("stops logging once the handlers are removed", async () => {
    const ipc = recordIPC();
    installGlobalErrorHandlers()();

    window.dispatchEvent(new ErrorEvent("error", { error: new Error("ignored") }));
    window.dispatchEvent(rejectionEvent(new Error("ignored")));
    await Promise.resolve();

    expect(ipc.logged()).toEqual([]);
  });

  it("does not throw when the log cannot be reached", () => {
    recordIPC(() => {
      throw new Error("backend unavailable");
    });
    uninstall = installGlobalErrorHandlers();

    expect(() => window.dispatchEvent(rejectionEvent(new Error("boom")))).not.toThrow();
  });
});

describe("reportError", () => {
  it("logs the error with the given source and component stack", async () => {
    const ipc = recordIPC();

    reportError("react", new Error("render failed"), { componentStack: "\n    at Widget" });

    await vi.waitFor(() => {
      expect(ipc.logged()).toHaveLength(1);
    });
    expect(ipc.logged()[0]).toMatchObject({
      level: "error",
      source: "react",
      message: "Error: render failed",
      componentStack: "\n    at Widget",
    });
  });
});
