import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ClaudeIntegrationStore } from "../claude/integration";
import type { AgentState, Layout, Session } from "../lib/ipc";
import { fakeClaude, INSTALLED } from "../test/fakeClaude";
import { fakeWorkspace } from "../test/fakeWorkspace";
import { session, singlePane, workspace } from "../test/workspaceFixtures";
import { WorkspaceStore } from "../workspace/workspaceStore";
import { AttentionMonitor, COOLDOWN_MS, SETTLE_MS } from "./monitor";
import { NotificationPrefs } from "./notificationPrefs";

const NOW = Date.parse("2026-11-27T12:00:00Z");

function agent(status: string, waitingSince: number | null = null, stale = false): AgentState {
  return {
    status,
    waitingSince,
    since: waitingSince ?? NOW,
    stale,
  } as AgentState;
}

const claudeSession = (id: string, overrides: Partial<Session> = {}) =>
  session(id, {
    kind: "claude",
    title: `Task ${id}`,
    state: { status: "running" },
    agent: agent("working"),
    ...overrides,
  });

/** `s-1` on screen; `s-2` and `s-3` are not. */
const shown: Layout = singlePane("s-1");

async function setup(
  options: {
    focused?: boolean;
    sessions?: Session[];
    claude?: ReturnType<typeof fakeClaude>;
  } = {},
) {
  const sessions = options.sessions ?? [
    claudeSession("s-1"),
    claudeSession("s-2"),
    claudeSession("s-3"),
  ];
  const fake = fakeWorkspace(workspace(sessions, { layout: shown }));
  const store = new WorkspaceStore(fake.backend);
  const claude = options.claude ?? fakeClaude();
  const integration = new ClaudeIntegrationStore(claude.backend);
  const prefs = new NotificationPrefs();
  const announced: string[] = [];
  let focused = options.focused ?? true;
  const monitor = new AttentionMonitor({
    workspace: store,
    claude: integration,
    prefs,
    windowFocused: () => focused,
    announce: (text) => announced.push(text),
  });
  monitor.start();
  await store.start();
  await integration.load();
  const update = (...changed: Session[]) => {
    fake.push({ sessions: changed });
  };
  const notifications = () => claude.callsOf("notify").map(([notification]) => notification);
  const badges = () => claude.callsOf("setAttentionBadge").map(([count]) => count);
  return {
    update,
    notifications,
    badges,
    announced,
    prefs,
    integration,
    claude,
    monitor,
    setFocused: (value: boolean) => {
      focused = value;
    },
  };
}

const waitingFor = (id: string, status = "waitingForPermission", since = Date.now()) =>
  claudeSession(id, { agent: agent(status, since) });

beforeEach(() => {
  vi.useFakeTimers({ now: NOW });
});

afterEach(() => {
  vi.useRealTimers();
  window.localStorage.clear();
});

describe("notifications", () => {
  it("fire once per wait, for a session not on screen, after it settled", async () => {
    const view = await setup();

    view.update(waitingFor("s-2"));
    expect(view.notifications()).toEqual([]);
    await vi.advanceTimersByTimeAsync(SETTLE_MS);

    expect(view.notifications()).toEqual([
      { title: "Task s-2", body: "Waiting for permission · app", sessionId: "s-2" },
    ]);

    // The same wait, sent again with another change: nothing new.
    view.update({ ...waitingFor("s-2", "waitingForPermission", NOW), title: "Task s-2" });
    await vi.advanceTimersByTimeAsync(COOLDOWN_MS * 2);
    expect(view.notifications()).toHaveLength(1);
  });

  it("stay quiet for the session the user looks at, but not when the window is in the background", async () => {
    const view = await setup();

    view.update(waitingFor("s-1"));
    await vi.advanceTimersByTimeAsync(SETTLE_MS);
    expect(view.notifications()).toEqual([]);
    // Screen readers still hear it.
    expect(view.announced).toEqual(["Task s-1 needs you: waiting for permission."]);

    view.setFocused(false);
    view.update(waitingFor("s-3", "waitingForInput"));
    await vi.advanceTimersByTimeAsync(SETTLE_MS);
    expect(view.notifications()).toEqual([
      { title: "Task s-3", body: "Waiting for input · app", sessionId: "s-3" },
    ]);
  });

  it("notify a session on screen while the window is unfocused", async () => {
    const view = await setup({ focused: false });

    view.update(waitingFor("s-1"));
    await vi.advanceTimersByTimeAsync(SETTLE_MS);

    expect(view.notifications()).toHaveLength(1);
  });

  it("ignore a wait that ends before it settles, and flapping", async () => {
    const view = await setup();

    view.update(waitingFor("s-2"));
    await vi.advanceTimersByTimeAsync(SETTLE_MS / 2);
    view.update(claudeSession("s-2"));
    await vi.advanceTimersByTimeAsync(SETTLE_MS);
    expect(view.notifications()).toEqual([]);

    view.update(waitingFor("s-2"));
    await vi.advanceTimersByTimeAsync(SETTLE_MS);
    expect(view.notifications()).toHaveLength(1);
    // Back to work and waiting again at once: no second notification.
    view.update(claudeSession("s-2"));
    view.update(waitingFor("s-2"));
    await vi.advanceTimersByTimeAsync(SETTLE_MS);
    expect(view.notifications()).toHaveLength(1);

    // A real new wait, later, is notified.
    await vi.advanceTimersByTimeAsync(COOLDOWN_MS);
    view.update(claudeSession("s-2"));
    view.update(waitingFor("s-2"));
    await vi.advanceTimersByTimeAsync(SETTLE_MS);
    expect(view.notifications()).toHaveLength(2);
  });

  it("coalesce a burst into one", async () => {
    const view = await setup({ focused: false });

    view.update(waitingFor("s-3", "waitingForInput", NOW - 1_000));
    await vi.advanceTimersByTimeAsync(500);
    view.update(waitingFor("s-1", "waitingForPermission", NOW - 60_000), waitingFor("s-2"));
    await vi.advanceTimersByTimeAsync(SETTLE_MS);

    expect(view.notifications()).toEqual([
      {
        title: "3 sessions need you",
        body: "Task s-1, Task s-3 and Task s-2",
        // The one waiting longest.
        sessionId: "s-1",
      },
    ]);
    expect(view.announced).toEqual(["3 sessions need you: Task s-1, Task s-3 and Task s-2."]);
  });

  it("respect the toggle, and say nothing at all of done unless asked", async () => {
    const view = await setup({ focused: false });

    view.prefs.set({ waiting: false });
    view.update(waitingFor("s-2"));
    view.update(claudeSession("s-3", { agent: agent("done") }));
    await vi.advanceTimersByTimeAsync(SETTLE_MS);
    expect(view.notifications()).toEqual([]);
    // The badge still counts.
    expect(view.badges().at(-1)).toBe(1);

    view.prefs.set({ waiting: true, done: true });
    view.update(claudeSession("s-1", { agent: agent("done") }));
    await vi.advanceTimersByTimeAsync(SETTLE_MS);
    expect(view.notifications()).toEqual([
      { title: "Task s-1", body: "Done · app", sessionId: "s-1" },
    ]);
  });

  it("notify nothing about the waits found at start-up", async () => {
    const view = await setup({
      focused: false,
      sessions: [waitingFor("s-1"), waitingFor("s-2"), claudeSession("s-3")],
    });

    await vi.advanceTimersByTimeAsync(SETTLE_MS * 2);

    expect(view.notifications()).toEqual([]);
    expect(view.badges()).toEqual([2]);
  });

  it("say nothing while the plugin does not report", async () => {
    const view = await setup({
      focused: false,
      claude: fakeClaude({ ...INSTALLED, plugin: "notInstalled", consent: "declined" }),
    });

    view.update(waitingFor("s-2"));
    await vi.advanceTimersByTimeAsync(SETTLE_MS);

    expect(view.notifications()).toEqual([]);
    expect(view.announced).toEqual([]);
    expect(view.badges()).toEqual([0]);
  });

  it("ignore stale reports", async () => {
    const view = await setup({ focused: false });

    view.update(claudeSession("s-2", { agent: agent("waitingForPermission", NOW, true) }));
    await vi.advanceTimersByTimeAsync(SETTLE_MS);

    expect(view.notifications()).toEqual([]);
    expect(view.badges()).toEqual([0]);
  });
});

describe("the badge", () => {
  it("holds the number of sessions that wait, sent when it changes", async () => {
    const view = await setup();

    view.update(waitingFor("s-1"), waitingFor("s-2"));
    view.update(waitingFor("s-3"));
    view.update(claudeSession("s-1"));
    view.update({ ...claudeSession("s-1"), title: "renamed" });

    expect(view.badges()).toEqual([0, 2, 3, 2]);
  });
});
