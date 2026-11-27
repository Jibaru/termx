import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { defaultBindings, PTY_RESERVED } from "./keybindings/keybindings";
import type { Layout, Project, Session } from "./lib/ipc";
import { flush, renderApp } from "./test/fakeWorkspace";
import { session, singlePane, workspace } from "./test/workspaceFixtures";

const app: Project = { key: "/work/app", name: "app", root: "/work/app", kind: "git" };
const api: Project = { key: "/work/api", name: "api", root: "/work/api", kind: "git" };

const running = (id: string, overrides: Partial<Session> = {}) =>
  session(id, { state: { status: "running" }, terminalId: 1, ...overrides });

function rows(): HTMLElement[] {
  return screen.queryAllByTestId("session-row");
}

function row(id: string): HTMLElement {
  const element = rows().find((each) => each.dataset.sessionId === id);
  if (!element) throw new Error(`no row for ${id}`);
  return element;
}

function nonNull<T>(value: T | null | undefined): T {
  if (value === null || value === undefined) throw new Error("missing element");
  return value;
}

function rowTitles(): string[] {
  return rows().map((each) => each.querySelector(".row-title")?.textContent ?? "");
}

function key(target: EventTarget, init: KeyboardEventInit) {
  const event = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
  act(() => {
    target.dispatchEvent(event);
  });
  return event;
}

const twoPanes: Layout = {
  root: {
    type: "split",
    id: 10,
    direction: "row",
    ratio: 0.5,
    first: { type: "pane", id: 1, session: "s-1" },
    second: { type: "pane", id: 2, session: "s-2" },
  },
  focused: 1,
  nextId: 11,
};

afterEach(() => {
  vi.useRealTimers();
  window.localStorage.clear();
});

describe("the window", () => {
  it("renders the sidebar, the app name and the core version", async () => {
    await renderApp(workspace([running("s-1")]));

    expect(screen.getByRole("complementary", { name: "Sessions" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: "termx" })).toBeInTheDocument();
    expect(await screen.findByText("core v0.0.0")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open log folder" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Export diagnostics" })).toBeInTheDocument();
  });

  it("opens a shell in an empty workspace", async () => {
    const view = await renderApp(workspace());

    expect(view.callsOf("createSession")).toEqual([[{ kind: "shell" }]]);
  });

  it("shows the session of the last run in its pane, waiting to be started", async () => {
    const view = await renderApp(workspace([session("s-last", { state: { status: "dormant" } })]));

    expect(view.of("s-last")?.autoStart).toBe("dormant");
    expect(view.of("s-last")?.attach).toHaveBeenCalledTimes(1);
    expect(view.callsOf("createSession")).toEqual([]);
  });

  it("puts a session on screen when no pane shows one", async () => {
    const view = await renderApp(
      workspace([session("s-1", { state: { status: "dormant" } })], { layout: singlePane(null) }),
    );

    expect(view.callsOf("showSession")).toEqual([[1, "s-1"]]);
  });
});

describe("the sidebar", () => {
  it("groups the sessions by project in the order of the workspace, background last", async () => {
    const snapshot = workspace([
      running("s-1", { title: "Fix the login", project: app }),
      running("s-2", { title: "API shell", project: api }),
      running("s-3", { title: "Review", project: app, kind: "claude" }),
      running("s-4", { title: "npm run dev", project: app, kind: "background" }),
    ]);
    snapshot.projects = [api, app];
    await renderApp(snapshot);

    const groups = screen.getAllByTestId("project-group");
    expect(groups.map((group) => group.dataset.projectKey)).toEqual(["/work/api", "/work/app"]);
    expect(rowTitles()).toEqual(["API shell", "Fix the login", "Review", "npm run dev"]);
    const background = screen.getByTestId("background-group");
    expect(within(background).getByText("npm run dev")).toBeInTheDocument();
    // In the background section a row names its project.
    expect(within(background).getByText("app")).toBeInTheDocument();
  });

  it("shows what each row needs to be told apart", async () => {
    await renderApp(
      workspace([
        running("s-1", {
          kind: "claude",
          title: "A long title that the sidebar wraps on two lines rather than cutting short",
          git: { branch: "feature/sidebar", commit: null, worktree: "termx-sidebar" },
          lastActivityAt: Date.now() - 5 * 60_000,
        }),
        session("s-2", {
          state: {
            status: "exited",
            code: 127,
            signal: null,
            failure: { kind: "claudeNotFound", message: "claude was not found" },
          },
        }),
        session("s-3", {
          state: {
            status: "exited",
            code: null,
            signal: null,
            failure: { kind: "folderMissing", message: "/gone does not exist" },
          },
        }),
        running("s-4", { warning: { kind: "launchUnconfirmed", message: "the profile hangs" } }),
        session("s-5", { state: { status: "exited", code: 3, signal: null, failure: null } }),
      ]),
    );

    const first = row("s-1");
    expect(within(first).getByText(/A long title/)).toHaveAttribute(
      "title",
      "A long title that the sidebar wraps on two lines rather than cutting short",
    );
    expect(within(first).getByText("feature/sidebar")).toBeInTheDocument();
    expect(within(first).getByTestId("worktree-marker")).toHaveTextContent("termx-sidebar");
    expect(within(first).getByTestId("session-status")).toHaveTextContent("Running");
    expect(within(first).getByText("5m")).toBeInTheDocument();
    expect(first.querySelector(".kind-claude")).not.toBeNull();
    expect(within(row("s-2")).getByTestId("session-status")).toHaveTextContent(
      "Claude Code is not installed",
    );
    expect(within(row("s-3")).getByTestId("session-status")).toHaveTextContent(
      "Folder no longer exists",
    );
    expect(within(row("s-4")).getByTestId("session-status")).toHaveTextContent(
      "Your shell profile did not run the command",
    );
    expect(within(row("s-5")).getByTestId("session-status")).toHaveTextContent(
      "Exited with code 3",
    );
  });

  it("shows each state as an icon of its own shape, not a coloured dot", async () => {
    await renderApp(
      workspace([
        running("s-1"),
        session("s-2", { state: { status: "dormant" } }),
        session("s-3", { state: { status: "exited", code: 1, signal: null, failure: null } }),
      ]),
    );

    const shape = (id: string) =>
      within(row(id)).getByTestId("session-status").querySelector("svg")?.dataset.shape;
    expect([shape("s-1"), shape("s-2"), shape("s-3")]).toEqual([
      "status-running",
      "status-dormant",
      "status-exitedError",
    ]);
    expect(document.querySelector(".status-dot")).toBeNull();
  });

  it("keeps the agent slot empty while the agent reports nothing, and loud while it waits", async () => {
    const view = await renderApp(workspace([running("s-1"), running("s-2")]));

    expect(within(row("s-1")).getByTestId("agent-slot")).toHaveAttribute("data-status", "none");
    expect(within(row("s-1")).getByTestId("agent-slot")).toBeEmptyDOMElement();

    view.update(
      running("s-1", {
        agent: {
          status: "waitingForPermission",
          waitingSince: Date.now(),
          since: Date.now(),
          stale: false,
        },
      }),
      running("s-2", { agent: { status: "brandNewStatus", waitingSince: null } as never }),
    );

    expect(within(row("s-1")).getByTestId("agent-slot")).toHaveTextContent("Needs permission");
    expect(row("s-1")).toHaveAttribute("data-attention", "true");
    const unknown = within(row("s-2")).getByTestId("agent-slot");
    expect(unknown).toHaveAttribute("data-status", "unknown");
    expect(unknown).toHaveTextContent("Unknown");
    expect(row("s-2")).not.toHaveAttribute("data-attention");
  });

  it("follows the deltas of the workspace", async () => {
    const view = await renderApp(workspace([running("s-1", { title: "zsh" }), running("s-2")]));

    view.update(running("s-1", { title: "vim notes.md", autoTitle: "vim notes.md" }));
    expect(rowTitles()[0]).toBe("vim notes.md");

    view.push({ removed: ["s-1"], sessionOrder: ["s-2"], layout: singlePane(null) });
    expect(rows()).toHaveLength(1);
    // The terminal of a closed session goes with it.
    expect(view.of("s-1")?.dispose).toHaveBeenCalledTimes(1);
  });

  it("shows a session on Enter, and moves with the arrow keys", async () => {
    const view = await renderApp(workspace([running("s-1"), running("s-2")]));
    row("s-1").focus();

    fireEvent.keyDown(row("s-1"), { key: "ArrowDown" });
    expect(document.activeElement).toBe(row("s-2"));
    fireEvent.keyDown(row("s-2"), { key: "Enter" });
    await flush();

    expect(view.callsOf("showSession")).toEqual([[1, "s-2"]]);
  });

  it("collapses a group and expands it again", async () => {
    await renderApp(workspace([running("s-1")]));
    const group = screen.getByTestId("project-group");

    group.focus();
    fireEvent.keyDown(group, { key: "ArrowLeft" });
    expect(group).toHaveAttribute("aria-expanded", "false");
    expect(rows()).toHaveLength(0);
    fireEvent.keyDown(group, { key: "ArrowRight" });
    expect(rows()).toHaveLength(1);
  });

  it("gives the focus to the pane that shows a session when its row is clicked", async () => {
    const view = await renderApp(workspace([running("s-1"), running("s-2")], { layout: twoPanes }));

    fireEvent.click(row("s-2"));
    await flush();

    expect(view.callsOf("focusPane")).toEqual([[2]]);
    expect(view.callsOf("showSession")).toEqual([]);
  });
});

describe("renaming", () => {
  it("names a session on double-click, and the name wins over the automatic title", async () => {
    const view = await renderApp(workspace([running("s-1", { title: "zsh", autoTitle: "zsh" })]));

    fireEvent.doubleClick(row("s-1"));
    const input = screen.getByTestId("rename-input");
    fireEvent.change(input, { target: { value: "  Deploy  " } });
    fireEvent.keyDown(input, { key: "Enter" });
    await flush();

    expect(view.callsOf("renameSession")).toEqual([["s-1", "Deploy"]]);
    view.update(running("s-1", { title: "Deploy", name: "Deploy", autoTitle: "vim" }));
    expect(rowTitles()).toEqual(["Deploy"]);
  });

  it("gives the automatic title back when the name is cleared", async () => {
    const view = await renderApp(
      workspace([running("s-1", { title: "Deploy", name: "Deploy", autoTitle: "zsh" })]),
    );

    row("s-1").focus();
    key(row("s-1"), { code: "F2", key: "F2" });
    const input = screen.getByTestId("rename-input");
    expect(input).toHaveValue("Deploy");
    expect(input).toHaveAttribute("placeholder", "zsh");
    fireEvent.change(input, { target: { value: "" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await flush();

    expect(view.callsOf("renameSession")).toEqual([["s-1", null]]);
    view.update(running("s-1", { title: "zsh", name: null, autoTitle: "zsh" }));
    expect(rowTitles()).toEqual(["zsh"]);
  });

  it("does not commit on the Enter that confirms what an input method composed", async () => {
    const view = await renderApp(workspace([running("s-1")]));

    fireEvent.doubleClick(row("s-1"));
    const input = screen.getByTestId("rename-input");
    fireEvent.change(input, { target: { value: "日本" } });
    const composing = new KeyboardEvent("keydown", {
      key: "Enter",
      bubbles: true,
      cancelable: true,
    });
    // happy-dom does not take these from the init dictionary.
    Object.defineProperty(composing, "isComposing", { value: true });
    Object.defineProperty(composing, "keyCode", { value: 229 });
    fireEvent(input, composing);
    await flush();

    expect(screen.getByTestId("rename-input")).toBeInTheDocument();
    expect(view.callsOf("renameSession")).toEqual([]);
    fireEvent.keyDown(input, { key: "Enter" });
    await flush();
    expect(view.callsOf("renameSession")).toEqual([["s-1", "日本"]]);
  });

  it("isolates titles written in the other direction", async () => {
    await renderApp(workspace([running("s-1", { title: "שלום" })]));

    expect(within(row("s-1")).getByText("שלום")).toHaveAttribute("dir", "auto");
  });

  it("changes nothing on Escape", async () => {
    const view = await renderApp(workspace([running("s-1")]));

    fireEvent.doubleClick(row("s-1"));
    const input = screen.getByTestId("rename-input");
    fireEvent.change(input, { target: { value: "Other" } });
    fireEvent.keyDown(input, { key: "Escape" });
    await flush();

    expect(screen.queryByTestId("rename-input")).toBeNull();
    expect(view.callsOf("renameSession")).toEqual([]);
  });

  it("is in the context menu", async () => {
    await renderApp(workspace([running("s-1")]));

    fireEvent.contextMenu(row("s-1"));
    fireEvent.click(screen.getByTestId("menu-rename"));

    expect(screen.getByTestId("rename-input")).toBeInTheDocument();
  });
});

describe("reordering", () => {
  const three = () =>
    workspace([running("s-1"), running("s-2"), running("s-3"), running("s-x", { project: api })]);

  it("moves a session within its group with Alt+arrows", async () => {
    const view = await renderApp(three());
    row("s-1").focus();

    fireEvent.keyDown(row("s-1"), { key: "ArrowDown", altKey: true });
    await flush();
    expect(view.callsOf("reorderSession")).toEqual([["s-1", "s-3"]]);

    row("s-3").focus();
    fireEvent.keyDown(row("s-3"), { key: "ArrowDown", altKey: true });
    await flush();
    // Already last of its group: nothing moves.
    expect(view.callsOf("reorderSession")).toHaveLength(1);

    fireEvent.keyDown(row("s-3"), { key: "ArrowUp", altKey: true });
    await flush();
    expect(view.callsOf("reorderSession")[1]).toEqual(["s-3", "s-2"]);
  });

  it("moves a session by dragging it onto another of its group", async () => {
    const view = await renderApp(three());
    const dataTransfer = { setData: vi.fn(), effectAllowed: "", dropEffect: "" };

    // happy-dom gives drag events no coordinates: the lower half of the
    // row is where the pointer is, so the session lands after it.
    fireEvent.dragStart(row("s-3"), { dataTransfer });
    fireEvent.dragOver(row("s-1"), { dataTransfer });
    expect(row("s-1")).toHaveAttribute("data-drop", "after");
    fireEvent.drop(row("s-1"), { dataTransfer });
    await flush();

    expect(view.callsOf("reorderSession")).toEqual([["s-3", "s-2"]]);
  });

  it("does not drop a session into another project", async () => {
    const view = await renderApp(three());
    const dataTransfer = { setData: vi.fn(), effectAllowed: "", dropEffect: "" };

    fireEvent.dragStart(row("s-1"), { dataTransfer });
    fireEvent.dragOver(row("s-x"), { dataTransfer });
    fireEvent.drop(row("s-x"), { dataTransfer });
    await flush();

    expect(view.callsOf("reorderSession")).toEqual([]);
  });

  it("moves a project group with Alt+arrows and by dragging", async () => {
    const snapshot = three();
    snapshot.projects = [app, api];
    const view = await renderApp(snapshot);
    const [appGroup, apiGroup] = screen.getAllByTestId("project-group");
    if (!appGroup || !apiGroup) throw new Error("two groups expected");

    appGroup.focus();
    fireEvent.keyDown(appGroup, { key: "ArrowDown", altKey: true });
    await flush();
    expect(view.callsOf("reorderProject")).toEqual([["/work/app", null]]);

    const dataTransfer = { setData: vi.fn(), effectAllowed: "", dropEffect: "" };
    const header = (group: HTMLElement) =>
      nonNull(group.querySelector<HTMLElement>(".group-header"));
    fireEvent.dragStart(header(appGroup), { dataTransfer });
    fireEvent.dragOver(header(apiGroup), { dataTransfer });
    fireEvent.drop(header(apiGroup), { dataTransfer });
    await flush();
    expect(view.callsOf("reorderProject")[1]).toEqual(["/work/app", null]);
  });
});

describe("shortcuts", () => {
  it("open the new-session dialog", async () => {
    await renderApp(workspace([running("s-1")]));

    key(window, { code: "KeyT", key: "T", ctrlKey: true, shiftKey: true });

    expect(screen.getByTestId("new-session-dialog")).toBeInTheDocument();
  });

  it("start another Claude session in the project of the focused one, without asking", async () => {
    const view = await renderApp(workspace([running("s-1"), running("s-2")]));

    key(window, { code: "KeyN", key: "N", ctrlKey: true, shiftKey: true });
    await flush();

    expect(view.callsOf("createSiblingSession")).toEqual([["s-1"]]);
    expect(screen.queryByTestId("new-session-dialog")).toBeNull();
  });

  it("ask before closing a session that runs", async () => {
    const view = await renderApp(workspace([running("s-1", { title: "Build" })]));

    key(window, { code: "KeyW", key: "W", ctrlKey: true, shiftKey: true });
    const dialog = screen.getByTestId("confirm-close-dialog");
    expect(dialog).toHaveTextContent("Close “Build”?");
    expect(view.callsOf("closeSession")).toEqual([]);
    fireEvent.click(screen.getByTestId("confirm-close"));
    await flush();

    expect(view.callsOf("closeSession")).toEqual([["s-1"]]);
    expect(screen.queryByTestId("confirm-close-dialog")).toBeNull();
  });

  it("close a session that does not run at once", async () => {
    const view = await renderApp(workspace([session("s-1", { state: { status: "dormant" } })]));

    key(window, { code: "KeyW", key: "W", ctrlKey: true, shiftKey: true });
    await flush();

    expect(view.callsOf("closeSession")).toEqual([["s-1"]]);
  });

  it("go to the next session and to session N", async () => {
    const view = await renderApp(workspace([running("s-1"), running("s-2"), running("s-3")]));

    key(window, { code: "BracketRight", key: "}", ctrlKey: true, shiftKey: true });
    await flush();
    expect(view.callsOf("showSession")).toEqual([[1, "s-2"]]);

    key(window, { code: "Digit3", key: "#", ctrlKey: true, shiftKey: true });
    await flush();
    expect(view.callsOf("showSession")[1]).toEqual([1, "s-3"]);
  });

  it("use Cmd on macOS", async () => {
    const view = await renderApp(workspace([running("s-1")]), { platform: "macos" });

    key(window, { code: "KeyD", key: "d", metaKey: true });
    await flush();

    expect(view.callsOf("splitPane")).toEqual([[1, "row"]]);
  });

  it("hide and show the sidebar", async () => {
    await renderApp(workspace([running("s-1")]));

    key(window, { code: "KeyB", key: "B", ctrlKey: true, shiftKey: true });
    expect(screen.queryByTestId("sidebar")).toBeNull();
    fireEvent.click(screen.getByTestId("show-sidebar"));
    expect(screen.getByTestId("sidebar")).toBeInTheDocument();
  });

  it.each(["macos", "linux", "windows"] as const)(
    "never take a key the program in the terminal needs (%s)",
    async (platform) => {
      const view = await renderApp(workspace([running("s-1")]), {
        platform,
        bindings: defaultBindings(platform),
      });
      // The hidden text area of xterm.js, where typed keys go.
      const xterm = document.createElement("div");
      xterm.className = "xterm";
      const textarea = document.createElement("textarea");
      xterm.appendChild(textarea);
      document.body.appendChild(xterm);
      const before = view.calls.length;

      const intercepted: string[] = [];
      act(() => {
        for (const chord of PTY_RESERVED) {
          const event = new KeyboardEvent("keydown", {
            bubbles: true,
            cancelable: true,
            code: chord.code,
            ctrlKey: chord.ctrl ?? false,
            shiftKey: chord.shift ?? false,
            altKey: chord.alt ?? false,
            metaKey: chord.meta ?? false,
          });
          textarea.dispatchEvent(event);
          if (event.defaultPrevented) intercepted.push(JSON.stringify(chord));
        }
      });
      expect(intercepted).toEqual([]);
      await flush();
      expect(view.calls.length).toBe(before);
      expect(screen.queryByTestId("rename-input")).toBeNull();

      // A shortcut is taken away from the terminal.
      const shortcut =
        platform === "macos"
          ? { code: "KeyT", metaKey: true, shiftKey: true }
          : { code: "KeyT", ctrlKey: true, shiftKey: true };
      expect(key(textarea, shortcut).defaultPrevented).toBe(true);
      xterm.remove();
    },
  );
});

describe("panes", () => {
  it("split the focused pane from its header", async () => {
    const view = await renderApp(workspace([running("s-1")]));

    fireEvent.click(screen.getByTestId("pane-split-right"));
    await flush();
    fireEvent.click(screen.getByTestId("pane-split-down"));
    await flush();

    expect(view.callsOf("splitPane")).toEqual([
      [1, "row"],
      [1, "column"],
    ]);
  });

  it("draw the layout of the workspace, one terminal per pane", async () => {
    const view = await renderApp(workspace([running("s-1"), running("s-2")], { layout: twoPanes }));

    const panes = screen.getAllByTestId("pane");
    expect(panes.map((pane) => pane.dataset.paneId)).toEqual(["1", "2"]);
    expect(panes[0]).toHaveAttribute("data-focused", "true");
    expect(view.of("s-1")?.attach).toHaveBeenCalledWith(expect.any(HTMLElement), { focus: true });
    expect(view.of("s-2")?.attach).toHaveBeenCalledWith(expect.any(HTMLElement), { focus: false });
    expect(screen.getByTestId("split")).toHaveAttribute("data-direction", "row");
  });

  it("give the focus to a pane that is clicked, or reached with the keyboard", async () => {
    const view = await renderApp(workspace([running("s-1"), running("s-2")], { layout: twoPanes }));

    fireEvent.mouseDown(nonNull(screen.getAllByTestId("pane")[1]));
    await flush();
    expect(view.callsOf("focusPane")).toEqual([[2]]);

    key(window, { code: "ArrowRight", ctrlKey: true, shiftKey: true });
    await flush();
    expect(view.callsOf("focusPane")).toEqual([[2], [2]]);

    view.push({ layout: { ...twoPanes, focused: 2 } });
    expect(view.of("s-2")?.focus).toHaveBeenCalled();
    key(window, { code: "ArrowLeft", ctrlKey: true, shiftKey: true });
    await flush();
    expect(view.callsOf("focusPane").at(-1)).toEqual([1]);
  });

  it("close, leaving the session running", async () => {
    const view = await renderApp(workspace([running("s-1"), running("s-2")], { layout: twoPanes }));

    const second = nonNull(screen.getAllByTestId("pane")[1]);
    fireEvent.click(within(second).getByTestId("pane-close"));
    await flush();

    expect(view.callsOf("closePane")).toEqual([[2]]);
    expect(view.callsOf("closeSession")).toEqual([]);
  });

  it("resize with the keyboard and by dragging the divider", async () => {
    const view = await renderApp(workspace([running("s-1"), running("s-2")], { layout: twoPanes }));
    const divider = screen.getByTestId("split-divider");

    fireEvent.keyDown(divider, { key: "ArrowRight" });
    await flush();
    expect(view.callsOf("resizeSplit")).toEqual([[10, 0.55]]);

    const split = screen.getByTestId("split");
    split.getBoundingClientRect = () =>
      ({ left: 0, top: 0, width: 1000, height: 500, right: 1000, bottom: 500 }) as DOMRect;
    divider.setPointerCapture = vi.fn();
    divider.hasPointerCapture = () => true;
    divider.releasePointerCapture = vi.fn();
    fireEvent.pointerDown(divider, { button: 0, pointerId: 1, clientX: 500 });
    fireEvent.pointerUp(divider, { pointerId: 1, clientX: 300 });
    await flush();

    expect(view.callsOf("resizeSplit").at(-1)).toEqual([10, 0.3]);
  });

  it("show a session from the sidebar in the focused pane", async () => {
    const view = await renderApp(
      workspace([running("s-1"), running("s-2"), running("s-3")], { layout: twoPanes }),
    );

    fireEvent.contextMenu(row("s-3"));
    fireEvent.click(screen.getByTestId("menu-show-in-focused-pane"));
    await flush();

    expect(view.callsOf("showSession")).toEqual([[1, "s-3"]]);
  });

  it("take a session dropped from the sidebar, in the middle or on an edge", async () => {
    const view = await renderApp(workspace([running("s-1"), running("s-2")]));
    const dataTransfer = { setData: vi.fn(), effectAllowed: "", dropEffect: "" };

    fireEvent.dragStart(row("s-2"), { dataTransfer });
    fireEvent.dragOver(screen.getByTestId("drop-zone-center"), { dataTransfer });
    fireEvent.drop(screen.getByTestId("drop-zone-center"), { dataTransfer });
    await flush();
    expect(view.callsOf("showSession")).toEqual([[1, "s-2"]]);

    fireEvent.dragStart(row("s-2"), { dataTransfer });
    fireEvent.drop(screen.getByTestId("drop-zone-right"), { dataTransfer });
    await flush();
    expect(view.callsOf("splitPane")).toEqual([[1, "row", { placement: "after", session: "s-2" }]]);
  });

  it("offer what can go in an empty pane", async () => {
    const view = await renderApp(
      workspace([running("s-1"), running("s-2")], {
        layout: {
          root: {
            type: "split",
            id: 10,
            direction: "row",
            ratio: 0.5,
            first: { type: "pane", id: 1, session: "s-1" },
            second: { type: "pane", id: 2, session: null },
          },
          focused: 2,
          nextId: 11,
        },
      }),
    );

    const empty = screen.getByTestId("empty-pane");
    fireEvent.click(within(empty).getByText("Shell"));
    await flush();

    expect(view.callsOf("showSession")).toEqual([[2, "s-2"]]);
  });
});

describe("restored sessions", () => {
  const restored = () =>
    workspace(
      [
        session("s-1", { kind: "claude", state: { status: "dormant" } }),
        session("s-2", { state: { status: "dormant" } }),
      ],
      { layout: singlePane("s-1") },
    );

  it("are announced in a banner that resumes them all", async () => {
    const view = await renderApp(restored());

    expect(screen.getByTestId("restore-banner")).toHaveTextContent(
      "2 sessions were restored from the last run and are not running.",
    );
    fireEvent.click(screen.getByTestId("resume-all"));

    expect(view.of("s-1")?.startDetached).toHaveBeenCalledExactlyOnceWith("resume");
    // The one that is not on screen too, without being shown.
    expect(view.of("s-2")?.startDetached).toHaveBeenCalledExactlyOnceWith("resume");
    expect(screen.queryByTestId("restore-banner")).toBeNull();
  });

  it("can be resumed one at a time, from the pane or the sidebar", async () => {
    const view = await renderApp(restored());

    expect(screen.getByTestId("pane-notice")).toHaveTextContent(
      "Resume carries on with its conversation.",
    );
    fireEvent.click(screen.getByTestId("pane-resume"));
    expect(view.of("s-1")?.startDetached).toHaveBeenCalledWith("resume");

    fireEvent.click(within(row("s-2")).getByTestId("row-resume"));
    expect(view.of("s-2")?.startDetached).toHaveBeenCalledWith("resume");

    view.update(running("s-1"), running("s-2"));
    expect(screen.queryByTestId("restore-banner")).toBeNull();
  });

  it("leave the banner once dismissed", async () => {
    await renderApp(restored());

    fireEvent.click(
      within(screen.getByTestId("restore-banner")).getByRole("button", { name: "Dismiss" }),
    );

    expect(screen.queryByTestId("restore-banner")).toBeNull();
  });
});

describe("after the page was reloaded", () => {
  it("attaches to every running session, shown or not, and calls none dormant", async () => {
    const view = await renderApp(
      workspace([running("s-shown", { terminalId: 3 }), running("s-hidden", { terminalId: 4 })]),
    );

    expect(view.of("s-shown")?.autoStart).toBe("reattach");
    // The one off the screen shows as running, and is attached shortly.
    expect(within(row("s-hidden")).getByTestId("session-status")).toHaveTextContent("Running");
    await waitFor(() => {
      expect(view.of("s-hidden")?.reattach).toHaveBeenCalledTimes(1);
    });
    for (const id of ["s-shown", "s-hidden"]) {
      expect(view.of(id)?.startDetached).not.toHaveBeenCalled();
    }
    expect(screen.queryByTestId("pane-notice")).toBeNull();
    expect(screen.queryByTestId("restore-banner")).toBeNull();
    expect(within(row("s-shown")).getByTestId("session-status")).toHaveTextContent("Running");
  });

  it("attaches to those on screen first, and to the others a few at a time", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const hidden = Array.from({ length: 10 }, (_, index) =>
      running(`s-h${String(index)}`, { terminalId: 10 + index }),
    );
    const view = await renderApp(workspace([running("s-shown", { terminalId: 3 }), ...hidden]));
    const attached = () => hidden.filter((each) => view.of(each.id)?.reattach.mock.calls.length);

    expect(view.of("s-shown")?.reattach).toHaveBeenCalledTimes(1);
    expect(attached()).toHaveLength(0);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(250);
    });
    expect(attached()).toHaveLength(4);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(500);
    });
    expect(attached()).toHaveLength(10);
  });

  it("never restarts a running session without asking", async () => {
    const view = await renderApp(workspace([running("s-1", { terminalId: 3, title: "Agent" })]));

    // Resuming a running session does nothing.
    view.workbench.resume("s-1");
    expect(view.of("s-1")?.startDetached).not.toHaveBeenCalled();

    fireEvent.contextMenu(row("s-1"));
    fireEvent.click(screen.getByTestId("menu-restart-running"));
    expect(screen.getByTestId("confirm-restart-dialog")).toHaveTextContent("Restart “Agent”?");
    expect(view.of("s-1")?.startDetached).not.toHaveBeenCalled();
    fireEvent.click(screen.getByTestId("confirm-restart"));

    expect(view.of("s-1")?.startDetached).toHaveBeenCalledExactlyOnceWith("fresh");
    expect(screen.queryByTestId("confirm-restart-dialog")).toBeNull();
  });
});

describe("the menu bar and quitting", () => {
  it("runs the command of a menu item as its shortcut would", async () => {
    const view = await renderApp(workspace([running("s-1")]), { platform: "macos" });

    act(() => {
      view.events.menu?.("splitRight");
      view.events.menu?.("noSuchCommand");
    });
    await flush();

    expect(view.callsOf("splitPane")).toEqual([[1, "row"]]);
  });

  it("gives the menu bar the shortcuts of the bindings", async () => {
    const view = await renderApp(workspace([running("s-1")]), { platform: "macos" });

    const [[shortcuts]] = view.callsOf("setMenuShortcuts") as [[{ command: string }[]]];
    expect(shortcuts).toContainEqual({ command: "splitRight", accelerator: "Cmd+KeyD" });
  });

  it("runs a key once when the menu bar sees it too", async () => {
    const view = await renderApp(workspace([running("s-1")]), { platform: "macos" });

    key(window, { code: "KeyD", key: "d", metaKey: true });
    act(() => {
      view.events.menu?.("splitRight");
    });
    await flush();

    expect(view.callsOf("splitPane")).toHaveLength(1);
  });

  it("keeps every key, the menu's included, while a name is typed", async () => {
    const view = await renderApp(workspace([running("s-1")]), { platform: "macos" });
    fireEvent.doubleClick(row("s-1"));
    const input = screen.getByTestId("rename-input");

    key(input, { code: "KeyB", key: "b", metaKey: true });
    key(input, { code: "KeyD", key: "d", metaKey: true });
    act(() => {
      view.events.menu?.("toggleSidebar");
      view.events.menu?.("splitRight");
    });
    await flush();

    expect(screen.getByTestId("sidebar")).toBeInTheDocument();
    expect(view.callsOf("splitPane")).toEqual([]);
    expect(screen.getByTestId("rename-input")).toBeInTheDocument();
  });

  it("closes a pane with Cmd+W, and never the only one", async () => {
    const view = await renderApp(workspace([running("s-1")]), { platform: "macos" });

    key(window, { code: "KeyW", key: "w", metaKey: true });
    await flush();
    expect(view.callsOf("closePane")).toEqual([]);

    view.push({ layout: twoPanes, sessions: [running("s-2")], sessionOrder: ["s-1", "s-2"] });
    key(window, { code: "KeyW", key: "w", metaKey: true });
    await flush();
    expect(view.callsOf("closePane")).toEqual([[1]]);
    expect(view.callsOf("closeSession")).toEqual([]);
  });

  it("asks before quitting while sessions run, and quits on yes", async () => {
    const view = await renderApp(workspace([running("s-1"), running("s-2")]));

    act(() => {
      view.events.quit?.(2);
    });
    expect(screen.getByTestId("confirm-quit-dialog")).toHaveTextContent(
      "2 sessions are running. Quitting stops them; they come back, not running",
    );
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await flush();
    expect(view.callsOf("quitApp")).toEqual([]);
    // The Rust side is told, so that the next close asks again.
    expect(view.callsOf("cancelQuit")).toEqual([[]]);

    act(() => {
      view.events.quit?.(2);
    });
    fireEvent.click(screen.getByTestId("confirm-quit"));
    await flush();
    expect(view.callsOf("quitApp")).toEqual([[]]);
  });

  it("shows the question again on a repeated quit, and never quits on it", async () => {
    const view = await renderApp(workspace([running("s-1")]));

    act(() => {
      view.events.quit?.(1);
    });
    await flush();
    // The Rust side is told it is on screen: a repeat now comes back here.
    expect(view.callsOf("quitShown")).toEqual([[]]);
    screen.getByRole("button", { name: "Cancel" }).focus();

    act(() => {
      view.events.quit?.(1);
    });
    await flush();

    expect(screen.getAllByTestId("confirm-quit-dialog")).toHaveLength(1);
    expect(document.activeElement).toBe(screen.getByTestId("confirm-quit"));
    expect(view.callsOf("quitApp")).toEqual([]);
  });
});

describe("saving", () => {
  it("warns, for as long as it lasts, that changes are not being saved", async () => {
    const view = await renderApp(workspace([running("s-1")]));
    expect(screen.queryByTestId("persistence-warning")).toBeNull();

    view.push({ persistence: { mode: "failing", reason: "the disk is full", since: 1 } });
    expect(screen.getByTestId("persistence-warning")).toHaveTextContent(
      "Changes are not being saved: the disk is full",
    );

    view.push({ persistence: { mode: "primary" } });
    expect(screen.queryByTestId("persistence-warning")).toBeNull();
  });

  it("says that a second window does not save", async () => {
    await renderApp(workspace([running("s-1")], { persistence: { mode: "secondary" } }));

    expect(screen.getByTestId("persistence-warning")).toHaveTextContent(
      "changes made here will not be saved",
    );
  });
});

describe("errors", () => {
  it("are shown in the words of their kind", async () => {
    await renderApp(workspace([running("s-1"), running("s-2")]), {
      responders: {
        showSession: () => {
          // eslint-disable-next-line @typescript-eslint/only-throw-error -- Tauri rejects with the serialised error.
          throw { command: "layout_show_session", kind: "notFound", message: "" };
        },
      },
    });

    fireEvent.click(row("s-2"));

    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent("It no longer exists");
    });
  });
});
