import { describe, expect, it, vi } from "vitest";

import { recordIPC } from "../test/ipcRecorder";
import {
  type CommandError,
  LOG_LIMIT,
  LOG_WINDOW_MS,
  describeError,
  exportDiagnostics,
  getCoreVersion,
  isCommandError,
  logFrontendEvent,
  openLogFolder,
} from "./ipc";

describe("command wrappers", () => {
  it("opens the log folder through the open_log_folder command", async () => {
    const ipc = recordIPC();

    await openLogFolder();

    expect(ipc.calls.map((call) => call.command)).toEqual(["open_log_folder"]);
  });

  it("exports the diagnostics without passing a destination", async () => {
    const ipc = recordIPC(() => ({ path: "/downloads/termx-diagnostics.zip" }));

    const bundle = await exportDiagnostics();

    expect(bundle).toEqual({ path: "/downloads/termx-diagnostics.zip" });
    expect(ipc.calls).toHaveLength(1);
    expect(ipc.calls[0]?.command).toBe("export_diagnostics");
    expect(JSON.stringify(ipc.calls[0]?.args ?? {})).toBe("{}");
  });
});

describe("failed IPC calls", () => {
  it("are logged with the command name and passed on to the caller", async () => {
    const ipc = recordIPC((command) => {
      if (command === "core_version") throw new Error("command core_version not found");
      return undefined;
    });

    await expect(getCoreVersion()).rejects.toThrow("command core_version not found");

    await vi.waitFor(() => {
      expect(ipc.logged()).toHaveLength(1);
    });
    expect(ipc.logged()[0]).toMatchObject({
      level: "error",
      source: "ipc",
      command: "core_version",
      message: "command core_version not found",
    });
  });

  it("are not logged again when the Rust side already logged a command error", async () => {
    const failure: CommandError = { command: "open_log_folder", message: "logging is not running" };
    const ipc = recordIPC((command) => {
      if (command === "open_log_folder") {
        // eslint-disable-next-line @typescript-eslint/only-throw-error -- Tauri rejects with the serialised error.
        throw failure;
      }
      return undefined;
    });

    await expect(openLogFolder()).rejects.toEqual(failure);
    await Promise.resolve();

    expect(ipc.logged()).toEqual([]);
  });
});

describe("logFrontendEvent", () => {
  it("sends the record through the log_frontend_event command", async () => {
    const ipc = recordIPC();

    await logFrontendEvent({ level: "warn", source: "test", message: "careful" });

    expect(ipc.calls).toEqual([
      {
        command: "log_frontend_event",
        args: { event: { level: "warn", source: "test", message: "careful" } },
      },
    ]);
  });

  it("never rejects, even when the backend is unavailable", async () => {
    recordIPC(() => {
      throw new Error("backend unavailable");
    });

    await expect(
      logFrontendEvent({ level: "error", source: "test", message: "boom" }),
    ).resolves.toBeUndefined();
  });

  it("drops records over the limit so an error loop cannot flood the log", async () => {
    vi.useFakeTimers();
    try {
      const ipc = recordIPC();

      for (let index = 0; index < LOG_LIMIT * 10; index += 1) {
        await logFrontendEvent({
          level: "error",
          source: "loop",
          message: `error ${String(index)}`,
        });
      }
      expect(ipc.logged()).toHaveLength(LOG_LIMIT);

      // A new window lets records through again.
      vi.advanceTimersByTime(LOG_WINDOW_MS);
      await logFrontendEvent({ level: "error", source: "loop", message: "later" });
      expect(ipc.logged()).toHaveLength(LOG_LIMIT + 1);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("describeError", () => {
  it("gives a readable message for anything that can be thrown", () => {
    expect(describeError(new Error("boom"))).toBe("boom");
    expect(describeError(new TypeError(""))).toBe("TypeError");
    expect(describeError("plain text")).toBe("plain text");
    expect(describeError({ command: "x", message: "from rust" })).toBe("from rust");
    expect(describeError({ code: 7 })).toBe('{"code":7}');
    expect(describeError(undefined)).toBe("undefined");
  });

  it("recognises the error of a Rust command", () => {
    expect(isCommandError({ command: "x", message: "y" })).toBe(true);
    expect(isCommandError(new Error("y"))).toBe(false);
    expect(isCommandError(null)).toBe(false);
    expect(isCommandError("x")).toBe(false);
  });
});
