import { Channel } from "@tauri-apps/api/core";
import { describe, expect, it, vi } from "vitest";

import { recordIPC } from "../test/ipcRecorder";
import {
  ackSession,
  closeSession,
  type CommandError,
  createSession,
  describeError,
  exportDiagnostics,
  getCoreVersion,
  isCommandError,
  logFrontendEvent,
  openExternalUrl,
  openLogFolder,
  readClipboardText,
  resetFrontendLogLimit,
  resizeSession,
  type SessionExit,
  writeClipboardText,
  writeSession,
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

describe("session commands", () => {
  /** Creates a session and returns what its handlers received. */
  async function create(respond: (command: string, args: unknown) => unknown = () => undefined) {
    const ipc = recordIPC((command, args) =>
      command === "session_create"
        ? { id: 7, ackBatchBytes: 8192, windowsBuildNumber: null }
        : respond(command, args),
    );
    const output: Uint8Array[] = [];
    const exits: SessionExit[] = [];
    const created = await createSession(
      { cols: 120, rows: 40 },
      { onOutput: (bytes) => output.push(bytes), onExit: (exit) => exits.push(exit) },
    );
    const args = ipc.calls[0]?.args as {
      request: unknown;
      onEvent: { onmessage: (message: unknown) => void };
    };
    return { ipc, created, output, exits, request: args.request, channel: args.onEvent };
  }

  it("creates a session with a channel for its output", async () => {
    const { ipc, created, request, channel } = await create();

    expect(created).toEqual({ id: 7, ackBatchBytes: 8192, windowsBuildNumber: null });
    expect(ipc.calls[0]?.command).toBe("session_create");
    expect(request).toEqual({ cols: 120, rows: 40 });
    expect(channel).toBeInstanceOf(Channel);
  });

  it("hands the raw bytes of the channel to the output handler", async () => {
    const { channel, output, exits } = await create();
    const bytes = Uint8Array.from([0x1b, 0x5b, 0x31, 0x6d, 0xe2, 0x9c, 0x93, 0xff]);

    channel.onmessage(bytes.buffer);

    expect(output).toHaveLength(1);
    expect(output[0]).toBeInstanceOf(Uint8Array);
    expect(Array.from(output[0] ?? [])).toEqual(Array.from(bytes));
    expect(exits).toEqual([]);
  });

  it("accepts the bytes as an array of numbers, as the fallback transport sends them", async () => {
    const { channel, output } = await create();

    channel.onmessage([104, 105, 255]);

    expect(Array.from(output[0] ?? [])).toEqual([104, 105, 255]);
  });

  it("hands the exit message of the channel to the exit handler", async () => {
    const { channel, output, exits } = await create();

    channel.onmessage({ event: "exit", code: 3, signal: null });
    channel.onmessage({ event: "exit", code: null, signal: "Hangup" });

    expect(exits).toEqual([
      { code: 3, signal: null },
      { code: null, signal: "Hangup" },
    ]);
    expect(output).toEqual([]);
  });

  it("writes, resizes, acknowledges and closes by session id", async () => {
    const ipc = recordIPC();

    await writeSession(7, "ls\r");
    await writeSession(7, "\xa0\xff", true);
    await resizeSession(7, 132, 43);
    await ackSession(7, 8192);
    await closeSession(7);

    expect(ipc.calls).toEqual([
      { command: "session_write", args: { id: 7, data: "ls\r", binary: false } },
      { command: "session_write", args: { id: 7, data: "\xa0\xff", binary: true } },
      { command: "session_resize", args: { id: 7, cols: 132, rows: 43 } },
      { command: "session_ack", args: { id: 7, bytes: 8192 } },
      { command: "session_close", args: { id: 7 } },
    ]);
  });

  it("never logs what was typed when a write fails", async () => {
    const ipc = recordIPC((command) => {
      if (command === "session_write") throw new Error("the connection was lost");
      return undefined;
    });

    await expect(writeSession(7, "my-secret-password\r")).rejects.toThrow();

    await vi.waitFor(() => {
      expect(ipc.logged()).toHaveLength(1);
    });
    expect(JSON.stringify(ipc.logged())).not.toContain("my-secret-password");
  });
});

describe("system integration", () => {
  it("opens links through the opener plugin", async () => {
    const ipc = recordIPC();

    await openExternalUrl("https://example.com/docs");

    expect(ipc.calls).toHaveLength(1);
    expect(ipc.calls[0]?.command).toBe("plugin:opener|open_url");
    expect(ipc.calls[0]?.args).toMatchObject({ url: "https://example.com/docs" });
  });

  it("reads and writes the clipboard through the clipboard plugin", async () => {
    const ipc = recordIPC((command) =>
      command === "plugin:clipboard-manager|read_text" ? "from the clipboard" : undefined,
    );

    await writeClipboardText("to the clipboard");
    const text = await readClipboardText();

    expect(text).toBe("from the clipboard");
    expect(ipc.calls.map((call) => call.command)).toEqual([
      "plugin:clipboard-manager|write_text",
      "plugin:clipboard-manager|read_text",
    ]);
    expect(ipc.calls[0]?.args).toMatchObject({ text: "to the clipboard" });
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
