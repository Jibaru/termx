import { describe, expect, it } from "vitest";

import type { AgentState } from "../lib/ipc";
import { session } from "../test/workspaceFixtures";
import {
  describeAgent,
  describeCommandError,
  describeState,
  folderName,
  agentStatusOf,
  persistenceMessage,
  statusOf,
  relativeTime,
  relativeTimeLong,
} from "./describe";

describe("the state of a session", () => {
  it("is named for each status", () => {
    expect(describeState(session("s", { state: { status: "dormant" } })).label).toBe("Not running");
    expect(describeState(session("s", { state: { status: "starting" } })).label).toBe("Starting");
    expect(describeState(session("s", { state: { status: "running" } }))).toEqual({
      label: "Running",
      tone: "ok",
    });
  });

  it("gives the exit code of a process that failed", () => {
    expect(
      describeState(
        session("s", { state: { status: "exited", code: 2, signal: null, failure: null } }),
      ),
    ).toEqual({ label: "Exited with code 2", tone: "error" });
    expect(
      describeState(
        session("s", { state: { status: "exited", code: 0, signal: null, failure: null } }),
      ).label,
    ).toBe("Exited");
  });

  it("says what went wrong in words for each kind of failure", () => {
    const failed = (kind: "claudeNotFound" | "folderMissing" | "launchUnconfirmed") =>
      describeState(
        session("s", {
          state: {
            status: "exited",
            code: 127,
            signal: null,
            failure: { kind, message: "the long story" },
          },
        }),
      );
    expect(failed("claudeNotFound")).toEqual({
      label: "Claude Code is not installed",
      tone: "error",
      detail: "the long story",
    });
    expect(failed("folderMissing").label).toBe("Folder no longer exists");
    expect(failed("launchUnconfirmed").label).toBe("Your shell profile did not run the command");
  });

  it("shows the warning of a running session", () => {
    const text = describeState(
      session("s", {
        state: { status: "running" },
        warning: { kind: "launchUnconfirmed", message: "details" },
      }),
    );
    expect(text).toEqual({
      label: "Your shell profile did not run the command",
      tone: "warn",
      detail: "details",
    });
  });

  it("treats a status it does not know as running", () => {
    const unknown = session("s", { state: { status: "hibernating" } as never });
    expect(describeState(unknown).label).toBe("Running");
  });
});

describe("the status icon of a session", () => {
  const exited = (code: number | null, extra: object = {}) =>
    session("s", { state: { status: "exited", code, signal: null, failure: null, ...extra } });

  it("follows the state of its process", () => {
    expect(statusOf(session("s", { state: { status: "dormant" } }))).toBe("dormant");
    expect(statusOf(session("s", { state: { status: "starting" } }))).toBe("starting");
    expect(statusOf(session("s", { state: { status: "running" } }))).toBe("running");
    expect(
      statusOf(
        session("s", {
          state: { status: "running" },
          warning: { kind: "launchUnconfirmed", message: "" },
        }),
      ),
    ).toBe("warning");
    expect(statusOf(exited(0))).toBe("exitedOk");
    expect(statusOf(exited(null))).toBe("exitedOk");
    expect(statusOf(exited(2))).toBe("exitedError");
    expect(statusOf(exited(null, { signal: "Hangup" }))).toBe("exitedError");
    expect(statusOf(exited(127, { failure: { kind: "claudeNotFound", message: "" } }))).toBe(
      "failed",
    );
    expect(statusOf(session("s", { state: { status: "hibernating" } as never }))).toBe("running");
  });

  it("shows an agent state it does not know as unknown", () => {
    expect(
      agentStatusOf({ status: "waitingForInput", waitingSince: 1, since: 1, stale: false }),
    ).toBe("waitingForInput");
    expect(agentStatusOf({ status: "compacting", waitingSince: null } as never)).toBe("unknown");
  });
});

describe("the agent of a session", () => {
  const agent = (status: string): AgentState =>
    ({ status, waitingSince: null }) as unknown as AgentState;

  it("says nothing while it reports nothing", () => {
    expect(describeAgent(null)).toBeNull();
  });

  it("is loud while it waits for the user", () => {
    expect(describeAgent(agent("waitingForPermission"))).toMatchObject({
      label: "Needs permission",
      tone: "attention",
    });
    expect(describeAgent(agent("waitingForInput"))?.tone).toBe("attention");
    expect(describeAgent(agent("working"))?.tone).toBe("busy");
    expect(describeAgent(agent("done"))?.tone).toBe("ok");
  });

  it("shows a status it does not know as unknown, and never loud", () => {
    expect(describeAgent(agent("compacting"))).toEqual({
      label: "Unknown",
      tone: "neutral",
      unknown: true,
    });
  });
});

describe("command errors", () => {
  it("are told in the words of their kind", () => {
    expect(
      describeCommandError({ command: "session_create", kind: "folderMissing", message: "" }),
    ).toBe("Folder no longer exists");
    expect(
      describeCommandError({
        command: "session_create",
        kind: "claudeNotFound",
        message: "Install it from claude.ai",
      }),
    ).toBe("Claude Code is not installed: Install it from claude.ai");
  });

  it("show the message of a kind with no words of its own", () => {
    expect(
      describeCommandError({ command: "x", kind: "internal", message: "something broke" }),
    ).toBe("something broke");
    expect(describeCommandError({ command: "x", kind: "brandNew", message: "new" })).toBe("new");
    expect(describeCommandError(new Error("plain"))).toBe("plain");
  });
});

describe("times and names", () => {
  it("say how long ago in a few characters", () => {
    expect(relativeTime(100_000, 95_000)).toBe("now");
    expect(relativeTime(100_000, 70_000)).toBe("30s");
    expect(relativeTime(10 * 60_000, 0)).toBe("10m");
    expect(relativeTime(5 * 3_600_000, 0)).toBe("5h");
    expect(relativeTime(3 * 86_400_000, 0)).toBe("3d");
    expect(relativeTimeLong(60_000, 0)).toBe("1 minute ago");
  });

  it("name a folder by its last part", () => {
    expect(folderName("/work/app/")).toBe("app");
    expect(folderName("C:\\Users\\dev\\api")).toBe("api");
  });

  it("say when changes are not saved", () => {
    expect(persistenceMessage({ mode: "primary" })).toBeNull();
    expect(persistenceMessage({ mode: "failing", reason: "the disk is full", since: 0 })).toBe(
      "Changes are not being saved: the disk is full",
    );
    expect(persistenceMessage({ mode: "secondary" })).toContain("second termx window");
  });
});
