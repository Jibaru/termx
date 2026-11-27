import { describe, expect, it } from "vitest";

import type { AgentState, Session } from "../lib/ipc";
import { session, workspace } from "../test/workspaceFixtures";
import {
  agentTooltip,
  attentionQueue,
  formatDuration,
  isWaiting,
  toMillis,
  viewAgent,
  waitKey,
} from "./agent";

const NOW = Date.parse("2026-11-27T12:00:00Z");
const minutesAgo = (minutes: number) => NOW - minutes * 60_000;

const running = (id: string, agent: AgentState | null, overrides: Partial<Session> = {}) =>
  session(id, { state: { status: "running" }, agent, title: id, ...overrides });

describe("timestamps", () => {
  it("are milliseconds, or nothing", () => {
    expect(toMillis(NOW)).toBe(NOW);
    expect(toMillis(null)).toBeNull();
    expect(toMillis(undefined)).toBeNull();
    expect(toMillis(Number.NaN)).toBeNull();
  });
});

describe("what an agent state shows", () => {
  it("names each state, loud only while it waits", () => {
    const view = (status: string) =>
      viewAgent({ status, waitingSince: null, since: minutesAgo(1), stale: false } as AgentState);
    expect(view("working")).toMatchObject({ icon: "working", tone: "busy", waiting: false });
    expect(view("waitingForPermission")).toMatchObject({
      icon: "waitingForPermission",
      label: "Needs permission",
      longLabel: "Waiting for permission",
      tone: "attention",
      waiting: true,
    });
    expect(view("waitingForInput")).toMatchObject({ tone: "attention", waiting: true });
    expect(view("done")).toMatchObject({ icon: "done", tone: "ok", waiting: false });
    // Anything this version does not know, the contract's "unknown" included.
    for (const status of ["unknown", "brandNew"]) {
      expect(view(status)).toMatchObject({ icon: "unknown", tone: "neutral", waiting: false });
    }
    expect(viewAgent(null)).toBeNull();
  });

  it("keeps a stale report quiet, whatever it says", () => {
    const stale = viewAgent({
      status: "waitingForPermission",
      waitingSince: minutesAgo(5),
      since: minutesAgo(5),
      stale: true,
    });
    expect(stale).toMatchObject({ icon: "waitingForPermission", tone: "neutral", waiting: false });
    expect(stale && agentTooltip(stale, NOW)).toBe("Waiting for permission · last seen 5 min ago");
  });

  it("says how long in its tooltip", () => {
    const waiting = viewAgent({
      status: "waitingForPermission",
      waitingSince: minutesAgo(3),
      since: minutesAgo(3),
      stale: false,
    });
    expect(waiting && agentTooltip(waiting, NOW)).toBe("Waiting for permission · 3 min");
    const working = viewAgent({
      status: "working",
      waitingSince: null,
      since: minutesAgo(0.2),
      stale: false,
    });
    expect(working && agentTooltip(working, NOW)).toBe("Working · under 1 min");
    const old = viewAgent({
      status: "waitingForInput",
      waitingSince: NOW - 120 * 60_000,
      since: NOW - 120 * 60_000,
      stale: false,
    });
    expect(old && agentTooltip(old, NOW)).toBe("Waiting for input · 2 h");
  });

  it("writes durations for people", () => {
    expect(formatDuration(5_000)).toBe("under 1 min");
    expect(formatDuration(59 * 60_000)).toBe("59 min");
    expect(formatDuration(3 * 3_600_000)).toBe("3 h");
    expect(formatDuration(50 * 3_600_000)).toBe("2 d");
  });
});

describe("the sessions that need the user", () => {
  const waiting = (id: string, minutes: number, overrides: Partial<Session> = {}) =>
    running(
      id,
      {
        status: "waitingForInput",
        waitingSince: minutesAgo(minutes),
        since: minutesAgo(minutes),
        stale: false,
      },
      overrides,
    );

  it("are the fresh waits of running sessions", () => {
    expect(isWaiting(waiting("a", 1))).toBe(true);
    expect(
      isWaiting(running("b", { status: "working", waitingSince: null, since: NOW, stale: false })),
    ).toBe(false);
    expect(
      isWaiting(
        running("c", {
          status: "waitingForInput",
          waitingSince: minutesAgo(1),
          since: minutesAgo(1),
          stale: true,
        }),
      ),
    ).toBe(false);
    expect(isWaiting(waiting("d", 1, { state: { status: "dormant" } }))).toBe(false);
  });

  it("come the longest waiting first, then in the order of the sidebar", () => {
    const snapshot = workspace([
      waiting("recent", 1),
      running("busy", { status: "working", waitingSince: null, since: NOW, stale: false }),
      waiting("oldest", 30),
      waiting("tie-first", 10),
      waiting("tie-second", 10),
    ]);
    expect(attentionQueue(snapshot).map((each) => each.id)).toEqual([
      "oldest",
      "tie-first",
      "tie-second",
      "recent",
    ]);
  });

  it("tell one wait from the next", () => {
    expect(waitKey(waiting("a", 5))).not.toBe(waitKey(waiting("a", 1)));
    expect(waitKey(waiting("a", 5))).toBe(waitKey(waiting("a", 5)));
  });
});
