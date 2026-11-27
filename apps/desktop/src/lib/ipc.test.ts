import { Channel } from "@tauri-apps/api/core";
import { describe, expect, it, vi } from "vitest";

import { recordIPC } from "../test/ipcRecorder";
import { session, singlePane, workspace } from "../test/workspaceFixtures";
import {
  ackSession,
  applyWorkspaceDelta,
  closePane,
  closeSession,
  removeSession,
  stopSession,
  type CommandError,
  createSession,
  createSiblingSession,
  describeError,
  exportDiagnostics,
  focusPane,
  forgetRecentFolder,
  getCoreVersion,
  getWorkspaceSnapshot,
  isCommandError,
  logFrontendEvent,
  movePane,
  openExternalUrl,
  openLogFolder,
  pickFolder,
  readClipboardText,
  refreshGit,
  renameSession,
  reorderProject,
  reorderSession,
  reportSessionCwd,
  reportSessionTitle,
  resetFrontendLogLimit,
  resizeSession,
  resizeSplit,
  type SessionExit,
  showSession,
  splitPane,
  startSession,
  subscribeWorkspace,
  type WorkspaceDelta,
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
  /** Starts a session and returns what its handlers received. */
  async function start(respond: (command: string, args: unknown) => unknown = () => undefined) {
    const ipc = recordIPC((command, args) =>
      command === "session_start"
        ? { terminal: 7, ackBatchBytes: 8192, windowsBuildNumber: null }
        : respond(command, args),
    );
    const output: Uint8Array[] = [];
    const exits: SessionExit[] = [];
    const started = await startSession(
      "s-abc",
      { mode: "resume", cols: 120, rows: 40 },
      { onOutput: (bytes) => output.push(bytes), onExit: (exit) => exits.push(exit) },
    );
    const args = ipc.calls[0]?.args as {
      id: unknown;
      request: unknown;
      onEvent: { onmessage: (message: unknown) => void };
    };
    return { ipc, started, output, exits, args, channel: args.onEvent };
  }

  it("starts a session with a channel for its output", async () => {
    const { ipc, started, args, channel } = await start();

    expect(started).toEqual({ terminal: 7, ackBatchBytes: 8192, windowsBuildNumber: null });
    expect(ipc.calls[0]?.command).toBe("session_start");
    expect(args.id).toBe("s-abc");
    expect(args.request).toEqual({ mode: "resume", cols: 120, rows: 40 });
    expect(channel).toBeInstanceOf(Channel);
  });

  it("hands the raw bytes of the channel to the output handler", async () => {
    const { channel, output, exits } = await start();
    const bytes = Uint8Array.from([0x1b, 0x5b, 0x31, 0x6d, 0xe2, 0x9c, 0x93, 0xff]);

    channel.onmessage(bytes.buffer);

    expect(output).toHaveLength(1);
    expect(output[0]).toBeInstanceOf(Uint8Array);
    expect(Array.from(output[0] ?? [])).toEqual(Array.from(bytes));
    expect(exits).toEqual([]);
  });

  it("accepts the bytes as an array of numbers, as the fallback transport sends them", async () => {
    const { channel, output } = await start();

    channel.onmessage([104, 105, 255]);

    expect(Array.from(output[0] ?? [])).toEqual([104, 105, 255]);
  });

  it("hands the exit message of the channel to the exit handler", async () => {
    const { channel, output, exits } = await start();
    const failure = { kind: "claudeNotFound", message: "Claude Code is not installed." };

    channel.onmessage({ event: "exit", code: 3, signal: null });
    channel.onmessage({ event: "exit", code: null, signal: "Hangup" });
    channel.onmessage({ event: "exit", code: 113, signal: null, failure });

    expect(exits).toEqual([
      { code: 3, signal: null, failure: null },
      { code: null, signal: "Hangup", failure: null },
      { code: 113, signal: null, failure },
    ]);
    expect(output).toEqual([]);
  });

  it("writes, resizes and acknowledges by the number of the terminal", async () => {
    const ipc = recordIPC();

    await writeSession(7, "ls\r");
    await writeSession(7, "\xa0\xff", true);
    await resizeSession(7, 132, 43);
    await ackSession(7, 8192);

    expect(ipc.calls).toEqual([
      { command: "session_write", args: { id: 7, data: "ls\r", binary: false } },
      { command: "session_write", args: { id: 7, data: "\xa0\xff", binary: true } },
      { command: "session_resize", args: { id: 7, cols: 132, rows: 43 } },
      { command: "session_ack", args: { id: 7, bytes: 8192 } },
    ]);
  });

  it("creates, names, orders and closes sessions by their id", async () => {
    const ipc = recordIPC((command) =>
      command.startsWith("session_create") ? session("s-new") : undefined,
    );

    expect(await createSession({ kind: "shell" })).toEqual(session("s-new"));
    await createSession({
      kind: "background",
      cwd: "/work/app",
      command: "pnpm dev",
      name: "dev server",
      show: false,
    });
    expect(await createSiblingSession("s-abc")).toEqual(session("s-new"));
    await renameSession("s-abc", "Login work");
    await renameSession("s-abc", null);
    await reorderSession("s-abc", "s-def");
    await reorderSession("s-abc", null);
    await reorderProject("/work/app", "/work/api");
    await reportSessionTitle("s-abc", "\u2733 Fix the login");
    await reportSessionCwd("s-abc", "file://host/work/app");
    await closeSession("s-abc");
    await stopSession("s-abc");
    await removeSession("s-abc");

    expect(ipc.calls).toEqual([
      { command: "session_create", args: { request: { kind: "shell" } } },
      {
        command: "session_create",
        args: {
          request: {
            kind: "background",
            cwd: "/work/app",
            command: "pnpm dev",
            name: "dev server",
            show: false,
          },
        },
      },
      { command: "session_create_sibling", args: { id: "s-abc" } },
      { command: "session_rename", args: { id: "s-abc", name: "Login work" } },
      { command: "session_rename", args: { id: "s-abc", name: null } },
      { command: "session_reorder", args: { id: "s-abc", before: "s-def" } },
      { command: "session_reorder", args: { id: "s-abc", before: null } },
      { command: "project_reorder", args: { key: "/work/app", before: "/work/api" } },
      { command: "session_report_title", args: { id: "s-abc", title: "\u2733 Fix the login" } },
      { command: "session_report_cwd", args: { id: "s-abc", cwd: "file://host/work/app" } },
      { command: "session_close", args: { id: "s-abc" } },
      { command: "session_stop", args: { id: "s-abc" } },
      { command: "session_remove", args: { id: "s-abc" } },
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
    const failure: CommandError = {
      command: "open_log_folder",
      kind: "internal",
      message: "logging is not running",
    };
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
    expect(describeError({ command: "x", kind: "notFound", message: "from rust" })).toBe(
      "from rust",
    );
    expect(describeError({ code: 7 })).toBe('{"code":7}');
    expect(describeError(undefined)).toBe("undefined");
  });

  it("recognises the error of a Rust command", () => {
    expect(isCommandError({ command: "x", kind: "internal", message: "y" })).toBe(true);
    // Without a kind it is not one.
    expect(isCommandError({ command: "x", message: "y" })).toBe(false);
    expect(isCommandError(new Error("y"))).toBe(false);
    expect(isCommandError(null)).toBe(false);
    expect(isCommandError("x")).toBe(false);
  });
});

describe("workspace commands", () => {
  it("subscribes with a channel and hands its messages to the listener", async () => {
    const snapshot = workspace([session("s-a")]);
    const ipc = recordIPC(() => snapshot);
    const deltas: WorkspaceDelta[] = [];

    const received = await subscribeWorkspace((delta) => deltas.push(delta));

    expect(received).toEqual(snapshot);
    expect(ipc.calls[0]?.command).toBe("workspace_subscribe");
    const { onEvent } = ipc.calls[0]?.args as {
      onEvent: { onmessage: (message: unknown) => void };
    };
    expect(onEvent).toBeInstanceOf(Channel);
    const delta = emptyDelta({ revision: 2, removed: ["s-a"] });
    onEvent.onmessage(delta);
    expect(deltas).toEqual([delta]);
  });

  it("reads the workspace, refreshes Git, picks and forgets folders", async () => {
    const snapshot = workspace();
    const ipc = recordIPC((command) => {
      if (command === "workspace_snapshot") return snapshot;
      if (command === "folder_pick") return "/work/picked";
      if (command === "recent_folder_forget") return true;
      return undefined;
    });

    expect(await getWorkspaceSnapshot()).toEqual(snapshot);
    await refreshGit();
    expect(await pickFolder()).toBe("/work/picked");
    expect(await forgetRecentFolder("/work/old")).toBe(true);

    expect(ipc.calls.map((call) => call.command)).toEqual([
      "workspace_snapshot",
      "workspace_refresh_git",
      "folder_pick",
      "recent_folder_forget",
    ]);
    expect(ipc.calls[3]?.args).toEqual({ folder: "/work/old" });
  });

  it("resolves with null when the folder picker is dismissed", async () => {
    recordIPC(() => null);

    expect(await pickFolder()).toBeNull();
  });

  it("changes the layout by the ids of panes and splits", async () => {
    const ipc = recordIPC((command) => {
      if (command === "layout_split") return 4;
      if (command === "layout_resize_split") return 0.9;
      return undefined;
    });

    expect(await splitPane(1, "row")).toBe(4);
    await splitPane(1, "column", { placement: "before", session: "s-a" });
    await closePane(4);
    await movePane(4, 1, "top");
    expect(await resizeSplit(3, 7)).toBe(0.9);
    await focusPane(1);
    await showSession(1, "s-a");
    await showSession(1, null);

    expect(ipc.calls).toEqual([
      {
        command: "layout_split",
        args: { pane: 1, direction: "row", placement: null, session: null },
      },
      {
        command: "layout_split",
        args: { pane: 1, direction: "column", placement: "before", session: "s-a" },
      },
      { command: "layout_close_pane", args: { pane: 4 } },
      { command: "layout_move_pane", args: { pane: 4, target: 1, edge: "top" } },
      { command: "layout_resize_split", args: { split: 3, ratio: 7 } },
      { command: "layout_focus_pane", args: { pane: 1 } },
      { command: "layout_show_session", args: { pane: 1, session: "s-a" } },
      { command: "layout_show_session", args: { pane: 1, session: null } },
    ]);
  });
});

/** A delta that says nothing changed, with whatever differs. */
function emptyDelta(overrides: Partial<WorkspaceDelta> = {}): WorkspaceDelta {
  return {
    revision: 0,
    sessions: [],
    removed: [],
    sessionOrder: null,
    projects: null,
    layout: null,
    recentFolders: null,
    persistence: null,
    ...overrides,
  };
}

describe("applyWorkspaceDelta", () => {
  const a = session("s-a");
  const b = session("s-b");
  const before = workspace([a, b], { revision: 5, recentFolders: ["/work/app"] });

  it("replaces the sessions that changed and keeps the others as they are", () => {
    const renamed = { ...b, title: "renamed", name: "renamed" };

    const after = applyWorkspaceDelta(before, emptyDelta({ revision: 6, sessions: [renamed] }));

    expect(after.revision).toBe(6);
    expect(after.sessions).toEqual([a, renamed]);
    // The session that did not change is the same object, and so is every
    // part the delta said nothing about.
    expect(after.sessions[0]).toBe(a);
    expect(after.layout).toBe(before.layout);
    expect(after.projects).toBe(before.projects);
    expect(after.recentFolders).toBe(before.recentFolders);
    expect(after.persistence).toBe(before.persistence);
    // The snapshot given is not modified.
    expect(before.sessions).toEqual([a, b]);
    expect(before.revision).toBe(5);
  });

  it("adds, removes and reorders sessions as the order sent says", () => {
    const c = session("s-c", { project: { key: "/x", name: "x", root: "/x", kind: "folder" } });

    const added = applyWorkspaceDelta(
      before,
      emptyDelta({
        revision: 6,
        sessions: [c],
        sessionOrder: ["s-c", "s-a", "s-b"],
        projects: [c.project, a.project],
        recentFolders: ["/x", "/work/app"],
        layout: singlePane("s-c"),
      }),
    );
    expect(added.sessions.map((s) => s.id)).toEqual(["s-c", "s-a", "s-b"]);
    expect(added.projects).toEqual([c.project, a.project]);
    expect(added.recentFolders).toEqual(["/x", "/work/app"]);
    expect(added.layout).toEqual(singlePane("s-c"));

    const removed = applyWorkspaceDelta(
      added,
      emptyDelta({ revision: 7, removed: ["s-a"], sessionOrder: ["s-c", "s-b"] }),
    );
    expect(removed.sessions.map((s) => s.id)).toEqual(["s-c", "s-b"]);

    const reordered = applyWorkspaceDelta(
      removed,
      emptyDelta({ revision: 8, sessionOrder: ["s-b", "s-c"] }),
    );
    expect(reordered.sessions.map((s) => s.id)).toEqual(["s-b", "s-c"]);
  });

  it("copes with deltas that are late, repeated or incomplete", () => {
    const c = session("s-c");

    // A session that is new, without the order: it goes last.
    const added = applyWorkspaceDelta(before, emptyDelta({ revision: 6, sessions: [c] }));
    expect(added.sessions.map((s) => s.id)).toEqual(["s-a", "s-b", "s-c"]);

    // The same delta again changes nothing.
    expect(applyWorkspaceDelta(added, emptyDelta({ revision: 6, sessions: [c] }))).toEqual(added);

    // A delta older than the snapshot does not take the revision back.
    const late = applyWorkspaceDelta(added, emptyDelta({ revision: 2, removed: ["s-gone"] }));
    expect(late.revision).toBe(6);
    expect(late.sessions).toEqual(added.sessions);

    // An order that names a session that is unknown, or leaves one out:
    // what is unknown is skipped, and what was left out is dropped.
    const odd = applyWorkspaceDelta(
      added,
      emptyDelta({ revision: 7, sessionOrder: ["s-c", "s-unknown", "s-a"] }),
    );
    expect(odd.sessions.map((s) => s.id)).toEqual(["s-c", "s-a"]);
  });
});
