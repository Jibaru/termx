import { act, fireEvent, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { SETTLE_MS } from "./attention/monitor";
import type { AgentState, Layout, Project, Session } from "./lib/ipc";
import { fakeClaude, INSTALLED } from "./test/fakeClaude";
import { flush, renderApp } from "./test/fakeWorkspace";
import { session, workspace } from "./test/workspaceFixtures";

/**
 * The attention model of the window, with the Claude Code integration and
 * the workspace replaced: what rows, groups, panes and the queue show for
 * each state of an agent, the jump shortcut, and notifications.
 */

const app: Project = { key: "/work/app", name: "app", root: "/work/app", kind: "git" };
const api: Project = { key: "/work/api", name: "api", root: "/work/api", kind: "git" };

/**
 * `minutes` and a little more ago: the clock of the window is read when it
 * starts, which can be a moment before a state the test sends later.
 */
const minutesAgo = (minutes: number) => Date.now() - minutes * 60_000 - 20_000;

function agent(status: string, minutes = 0, stale = false): AgentState {
  const waiting = status.startsWith("waitingFor");
  return {
    status,
    waitingSince: waiting ? minutesAgo(minutes) : null,
    since: minutesAgo(minutes),
    stale,
  } as AgentState;
}

const claude = (id: string, overrides: Partial<Session> = {}) =>
  session(id, {
    kind: "claude",
    title: `Task ${id}`,
    state: { status: "running" },
    terminalId: 1,
    project: app,
    ...overrides,
  });

function row(id: string): HTMLElement {
  const element = screen
    .queryAllByTestId("session-row")
    .find((each) => each.dataset.sessionId === id);
  if (!element) throw new Error(`no row for ${id}`);
  return element;
}

function queueIds(): string[] {
  return screen.queryAllByTestId("attention-item").map((item) => item.dataset.sessionId ?? "");
}

function press(init: KeyboardEventInit) {
  act(() => {
    document.body.dispatchEvent(
      new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
    );
  });
}

/** Ctrl+Shift+A on Linux: go to the session waiting longest. */
const jump = () => {
  press({ code: "KeyA", key: "A", ctrlKey: true, shiftKey: true });
};

afterEach(() => {
  vi.useRealTimers();
  window.localStorage.clear();
});

describe("agent states in the sidebar", () => {
  it("shows each state with its own icon, and the unknown and the stale quietly", async () => {
    await renderApp(
      workspace([
        claude("s-work", { agent: agent("working", 2) }),
        claude("s-perm", { agent: agent("waitingForPermission", 3) }),
        claude("s-input", { agent: agent("waitingForInput", 1) }),
        claude("s-done", { agent: agent("done", 4) }),
        claude("s-unknown", { agent: agent("somethingNew", 1) }),
        claude("s-stale", { agent: agent("waitingForPermission", 5, true) }),
        claude("s-none", { agent: null }),
      ]),
    );

    const slot = (id: string) => within(row(id)).getByTestId("agent-slot");
    const shape = (id: string) => slot(id).querySelector("svg")?.dataset.shape;
    expect(shape("s-work")).toBe("status-working");
    expect(shape("s-perm")).toBe("status-waitingForPermission");
    expect(shape("s-input")).toBe("status-waitingForInput");
    expect(shape("s-done")).toBe("status-done");
    expect(shape("s-unknown")).toBe("status-unknown");
    expect(slot("s-none")).toHaveAttribute("data-status", "none");

    expect(slot("s-perm")).toHaveAttribute("title", "Waiting for permission · 3 min");
    expect(slot("s-perm")).toHaveTextContent("Needs permission · 3m");
    expect(slot("s-work")).toHaveAttribute("title", "Working · 2 min");
    expect(slot("s-unknown")).toHaveAttribute("data-tone", "neutral");
    expect(slot("s-stale")).toHaveAttribute("data-stale", "true");
    expect(slot("s-stale")).toHaveAttribute("data-tone", "neutral");
    expect(slot("s-stale")).toHaveAttribute(
      "title",
      "Waiting for permission · last seen 5 min ago",
    );
  });

  it("gives the attention treatment to fresh waits only", async () => {
    await renderApp(
      workspace([
        claude("s-work", { agent: agent("working") }),
        claude("s-perm", { agent: agent("waitingForPermission") }),
        claude("s-input", { agent: agent("waitingForInput") }),
        claude("s-done", { agent: agent("done") }),
        claude("s-unknown", { agent: agent("somethingNew") }),
        claude("s-stale", { agent: agent("waitingForInput", 5, true) }),
        claude("s-dormant", { agent: agent("waitingForInput"), state: { status: "dormant" } }),
      ]),
    );

    const loud = screen
      .getAllByTestId("session-row")
      .filter((each) => each.hasAttribute("data-attention"))
      .map((each) => each.dataset.sessionId);
    expect(loud).toEqual(["s-perm", "s-input"]);
    expect(row("s-perm").getAttribute("aria-label")).toMatch(/^Task s-perm, needs you,/);
  });

  it("counts the waits of a collapsed group on its header", async () => {
    await renderApp(
      workspace([
        claude("s-1", { agent: agent("waitingForInput") }),
        claude("s-2", { agent: agent("waitingForPermission") }),
        claude("s-3", { agent: agent("working") }),
        claude("s-4", { project: api, agent: agent("working") }),
      ]),
    );
    const [appGroup] = screen.getAllByTestId("project-group");
    if (!appGroup) throw new Error("no group");

    expect(within(appGroup).queryByTestId("group-attention")).toBeNull();
    fireEvent.click(nonNull(appGroup.querySelector(".group-header")));

    expect(within(appGroup).getByTestId("group-attention")).toHaveTextContent("2");
    expect(appGroup).toHaveAttribute("aria-label", "app, 3 sessions, 2 need you");
  });

  it("shows the state in the header of the pane too", async () => {
    const view = await renderApp(workspace([claude("s-1", { agent: agent("working") })]));
    const pane = screen.getByTestId("pane");

    expect(within(pane).getByTestId("pane-agent")).toHaveAttribute("data-status", "working");
    expect(pane).not.toHaveAttribute("data-attention");

    view.update(claude("s-1", { agent: agent("waitingForPermission", 2) }));
    expect(within(pane).getByTestId("pane-agent")).toHaveAttribute(
      "title",
      "Waiting for permission · 2 min",
    );
    expect(pane).toHaveAttribute("data-attention", "true");
  });

  it("shows no agent state while the plugin is not installed", async () => {
    await renderApp(workspace([claude("s-1", { agent: agent("waitingForPermission") })]), {
      claude: fakeClaude({ ...INSTALLED, plugin: "notInstalled", consent: "declined" }),
    });

    expect(within(row("s-1")).getByTestId("agent-slot")).toHaveAttribute("data-status", "none");
    expect(row("s-1")).not.toHaveAttribute("data-attention");
    expect(screen.queryByTestId("attention-queue")).toBeNull();
    expect(within(screen.getByTestId("pane")).getByTestId("pane-agent")).toBeEmptyDOMElement();
  });
});

describe("the attention queue", () => {
  it("lists the sessions that wait, the longest first, and follows the deltas", async () => {
    const view = await renderApp(
      workspace([
        claude("s-1", { agent: agent("waitingForInput", 1) }),
        claude("s-2", { agent: agent("working") }),
        claude("s-3", { agent: agent("waitingForPermission", 10) }),
      ]),
    );

    const queue = screen.getByTestId("attention-queue");
    expect(within(queue).getByRole("heading")).toHaveTextContent("Needs you (2)");
    expect(queueIds()).toEqual(["s-3", "s-1"]);

    view.update(claude("s-2", { agent: agent("waitingForInput", 5) }));
    expect(queueIds()).toEqual(["s-3", "s-2", "s-1"]);

    view.update(claude("s-3", { agent: agent("working") }));
    expect(queueIds()).toEqual(["s-2", "s-1"]);

    view.update(
      claude("s-1", { agent: agent("done") }),
      claude("s-2", { agent: agent("waitingForInput", 5, true) }),
    );
    expect(screen.queryByTestId("attention-queue")).toBeNull();
  });

  it("shows a session when its item is clicked, or chosen with the keyboard", async () => {
    const view = await renderApp(
      workspace([claude("s-1"), claude("s-2", { agent: agent("waitingForInput", 1) })]),
    );

    const [item] = screen.getAllByTestId("attention-item");
    if (!item) throw new Error("no item");
    expect(item.tagName).toBe("BUTTON");
    expect(item).toHaveAccessibleName("Task s-2, app: Waiting for input · 1 min");
    fireEvent.click(item);
    await flush();

    expect(view.callsOf("showSession")).toEqual([[1, "s-2"]]);
  });
});

describe("going to the sessions that wait", () => {
  const panes: Layout = {
    root: {
      type: "split",
      id: 10,
      direction: "row",
      ratio: 0.5,
      first: { type: "pane", id: 1, session: "s-1" },
      second: { type: "pane", id: 2, session: "s-4" },
    },
    focused: 1,
    nextId: 11,
  };

  it("goes to the one waiting longest, then to each of the others in turn", async () => {
    const view = await renderApp(
      workspace(
        [
          claude("s-1"),
          claude("s-2", { agent: agent("waitingForInput", 2) }),
          claude("s-3", { agent: agent("waitingForPermission", 9) }),
          claude("s-4", { agent: agent("waitingForInput", 5) }),
        ],
        { layout: panes },
      ),
    );

    jump();
    await flush();
    // Not on screen: shown in the focused pane.
    expect(view.callsOf("showSession")).toEqual([[1, "s-3"]]);
    view.push({ layout: { ...panes, root: withFirst(panes, "s-3") } });

    jump();
    await flush();
    // On screen in the other pane: that pane takes the focus.
    expect(view.callsOf("focusPane")).toEqual([[2]]);
    view.push({ layout: { ...panes, root: withFirst(panes, "s-3"), focused: 2 } });

    jump();
    await flush();
    expect(view.callsOf("showSession")).toEqual([
      [1, "s-3"],
      [2, "s-2"],
    ]);
  });

  it("goes to the next one in the order of the sidebar from the menu", async () => {
    const view = await renderApp(
      workspace([
        claude("s-1"),
        claude("s-2", { agent: agent("waitingForInput", 1) }),
        claude("s-3"),
        claude("s-4", { agent: agent("waitingForPermission", 9) }),
      ]),
    );

    act(() => {
      view.events.menu?.("nextWaitingSession");
    });
    await flush();

    expect(view.callsOf("showSession")).toEqual([[1, "s-2"]]);
  });

  it("does nothing but say so when nothing waits", async () => {
    const view = await renderApp(workspace([claude("s-1", { agent: agent("working") })]));

    jump();
    await flush();

    expect(view.callsOf("showSession")).toEqual([]);
    expect(view.callsOf("focusPane")).toEqual([]);
    expect(screen.getByTestId("hint")).toHaveTextContent("No session needs you right now.");
    expect(screen.getByTestId("live-region")).toHaveTextContent("No session needs you right now.");
  });

  it("is in the menu bar of macOS with its shortcut", async () => {
    const view = await renderApp(workspace([claude("s-1")]), { platform: "macos" });

    const [shortcuts] = view.callsOf("setMenuShortcuts")[0] as [
      { command: string; accelerator: string | null }[],
    ];
    expect(shortcuts).toContainEqual({
      command: "jumpToWaitingSession",
      accelerator: "Shift+Cmd+KeyA",
    });
    expect(shortcuts).toContainEqual({ command: "nextWaitingSession", accelerator: null });
  });
});

describe("notifications from the window", () => {
  it("are sent for a session out of sight, announced, and a click shows the session", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const fake = fakeClaude();
    const view = await renderApp(workspace([claude("s-1"), claude("s-2")]), { claude: fake });

    view.update(claude("s-2", { agent: agent("waitingForPermission") }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(SETTLE_MS);
    });

    expect(fake.callsOf("notify")).toEqual([
      [{ title: "Task s-2", body: "Waiting for permission · app", sessionId: "s-2" }],
    ]);
    expect(fake.callsOf("setAttentionBadge").at(-1)).toEqual([1]);
    expect(screen.getByTestId("live-region")).toHaveTextContent(
      "Task s-2 needs you: waiting for permission.",
    );

    act(() => {
      fake.click("s-2");
    });
    await flush();
    expect(view.callsOf("showSession")).toEqual([[1, "s-2"]]);
  });
});

function withFirst(layout: Layout, id: string): Layout["root"] {
  if (layout.root.type !== "split") throw new Error("not a split");
  return { ...layout.root, first: { type: "pane", id: 1, session: id } };
}

function nonNull<T>(value: T | null | undefined): T {
  if (value === null || value === undefined) throw new Error("missing element");
  return value;
}
