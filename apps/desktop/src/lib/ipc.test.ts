import { describe, expect, it, vi } from "vitest";

import { recordIPC } from "../test/ipcRecorder";
import {
  type CommandError,
  describeError,
  exportDiagnostics,
  getCoreVersion,
  isCommandError,
  logFrontendEvent,
  openLogFolder,
  resetFrontendLogLimit,
} from "./ipc";
import { DEFAULT_LOG_LIMITS } from "./logLimiter";

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
      message: "Error: command core_version not found",
    });
  });

  it("are classified, not copied, when Tauri rejects with a string", async () => {
    const ipc = recordIPC((command) => {
      if (command === "core_version") {
        // eslint-disable-next-line @typescript-eslint/only-throw-error -- Tauri rejects with a string.
        throw 'invalid args `data`: invalid type: string "typed-by-the-user", expected u32';
      }
      return undefined;
    });

    await expect(getCoreVersion()).rejects.toBeTypeOf("string");

    await vi.waitFor(() => {
      expect(ipc.logged()).toHaveLength(1);
    });
    expect(ipc.logged()[0]?.message).toMatch(/^invalid arguments \(string of length \d+\)$/);
    expect(JSON.stringify(ipc.logged())).not.toContain("typed-by-the-user");
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

  it("drops repeats over the limit, then logs how many were dropped", async () => {
    const clock = { now: 0 };
    resetFrontendLogLimit(DEFAULT_LOG_LIMITS, () => clock.now);
    const ipc = recordIPC();
    const limit = DEFAULT_LOG_LIMITS.perLevel.error;

    for (let index = 0; index < limit * 10; index += 1) {
      await logFrontendEvent({ level: "error", source: "loop", message: "the same error" });
    }
    expect(ipc.logged()).toHaveLength(limit);

    // A different error still gets through, and brings the count with it.
    await logFrontendEvent({ level: "error", source: "disk", message: "disk full" });
    const logged = ipc.logged();
    expect(logged).toHaveLength(limit + 2);
    expect(logged[limit]).toMatchObject({ level: "warn", source: "frontend-limit" });
    expect(logged[limit]?.message).toContain(`${String(limit * 9)} records were dropped`);
    expect(logged[limit + 1]).toMatchObject({ source: "disk", message: "disk full" });

    // A new window lets repeats through again.
    clock.now = DEFAULT_LOG_LIMITS.windowMs;
    await logFrontendEvent({ level: "error", source: "loop", message: "the same error" });
    expect(ipc.logged()).toHaveLength(limit + 3);
  });

  it("does not let low-level noise use up the room for errors", async () => {
    const ipc = recordIPC();

    for (let index = 0; index < 1_000; index += 1) {
      await logFrontendEvent({ level: "info", source: "noise", message: "tick" });
    }
    await logFrontendEvent({ level: "error", source: "test", message: "a real error" });

    expect(ipc.logged().filter((event) => event.level === "info")).toHaveLength(
      DEFAULT_LOG_LIMITS.perLevel.info,
    );
    expect(ipc.logged().some((event) => event.message === "a real error")).toBe(true);
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
