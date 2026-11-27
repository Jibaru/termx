import { describe, expect, it, vi } from "vitest";

import type { LaunchMode, NewSession, SessionId, WorkspaceSnapshot } from "../lib/ipc";
import { deferred } from "../test/terminalHarness";
import { session, singlePane, workspace } from "../test/workspaceFixtures";
import { SessionStore, sessionToRestore } from "./sessionStore";
import type { TerminalController } from "./TerminalController";

/**
 * A store whose terminals are stand-ins that only record how they were
 * made and their disposal, on a workspace that holds `restored`.
 */
function store(restored: WorkspaceSnapshot = workspace()) {
  const controllers: {
    sessionId: SessionId;
    autoStart: LaunchMode | "dormant";
    dispose: ReturnType<typeof vi.fn>;
  }[] = [];
  const created: NewSession[] = [];
  const closed: SessionId[] = [];
  const snapshots: (() => Promise<WorkspaceSnapshot>)[] = [];
  const backend = {
    getWorkspaceSnapshot: () => (snapshots.shift() ?? (() => Promise.resolve(restored)))(),
    createSession: (request: NewSession) => {
      created.push(request);
      return Promise.resolve(session(`s-new-${String(created.length)}`, { kind: request.kind }));
    },
    closeSession: (id: SessionId) => {
      closed.push(id);
      return Promise.resolve();
    },
  };
  const sessions = new SessionStore((sessionId, autoStart) => {
    const controller = { sessionId, autoStart, dispose: vi.fn() };
    controllers.push(controller);
    return controller as unknown as TerminalController;
  }, backend);
  return { sessions, controllers, created, closed, snapshots };
}

describe("SessionStore", () => {
  it("starts empty", () => {
    const { sessions } = store();

    expect(sessions.getSnapshot()).toEqual({ keys: [], activeKey: null });
  });

  it("opens a new shell in an empty workspace, once, however often it is asked", async () => {
    const { sessions, controllers, created } = store();

    const first = sessions.ensureOne();
    const second = sessions.ensureOne();
    await Promise.all([first, second]);
    await sessions.ensureOne();

    // No folder is given: the Rust side starts it in the home directory.
    expect(created).toEqual([{ kind: "shell" }]);
    expect(controllers).toHaveLength(1);
    expect(controllers[0]).toMatchObject({ sessionId: "s-new-1", autoStart: "fresh" });
    expect(sessions.getSnapshot()).toEqual({ keys: ["s-new-1"], activeKey: "s-new-1" });
    expect(sessions.get("s-new-1")).toBe(controllers[0]);
  });

  it("shows the session termx last had on screen, without starting it", async () => {
    const restored = workspace(
      [
        session("s-first", { state: { status: "dormant" } }),
        session("s-shown", { state: { status: "dormant" }, cwd: "/work/api" }),
      ],
      { layout: singlePane("s-shown") },
    );
    const { sessions, controllers, created } = store(restored);

    await sessions.ensureOne();

    expect(created).toEqual([]);
    expect(controllers).toHaveLength(1);
    expect(controllers[0]).toMatchObject({ sessionId: "s-shown", autoStart: "dormant" });
    expect(sessions.getSnapshot().activeKey).toBe("s-shown");
  });

  it("tries again after the workspace could not be read", async () => {
    const { sessions, controllers, snapshots } = store();
    snapshots.push(() => Promise.reject(new Error("the backend is not ready")));

    await sessions.ensureOne();
    expect(controllers).toHaveLength(0);

    await sessions.ensureOne();
    expect(controllers).toHaveLength(1);
  });

  it("does not open a second terminal when one was opened in the meantime", async () => {
    const { sessions, controllers, created, snapshots } = store();
    const slow = deferred<WorkspaceSnapshot>();
    snapshots.push(() => slow.promise);

    const pending = sessions.ensureOne();
    sessions.open("s-other");
    slow.resolve(workspace());
    await pending;

    expect(controllers.map((controller) => controller.sessionId)).toEqual(["s-other"]);
    expect(created).toEqual([]);
  });

  it("keeps one terminal per session and shows the one opened last", () => {
    const { sessions, controllers } = store();

    const first = sessions.open("s-a");
    const second = sessions.open("s-b", "resume");

    expect([first, second]).toEqual(["s-a", "s-b"]);
    expect(sessions.getSnapshot()).toEqual({ keys: ["s-a", "s-b"], activeKey: "s-b" });
    expect(sessions.get("s-a")).toBe(controllers[0]);
    expect(sessions.get("s-b")).toBe(controllers[1]);
    expect(controllers[1]?.autoStart).toBe("resume");

    sessions.activate("s-a");
    expect(sessions.getSnapshot().activeKey).toBe("s-a");
    sessions.activate("unknown");
    expect(sessions.getSnapshot().activeKey).toBe("s-a");

    // Opening a session that has a terminal only shows it.
    expect(sessions.open("s-b")).toBe("s-b");
    expect(controllers).toHaveLength(2);
    expect(sessions.getSnapshot()).toEqual({ keys: ["s-a", "s-b"], activeKey: "s-b" });
  });

  it("notifies its subscribers with a new snapshot for every change", () => {
    const { sessions } = store();
    const listener = vi.fn();
    const before = sessions.getSnapshot();
    const unsubscribe = sessions.subscribe(listener);

    const key = sessions.open("s-a");
    expect(listener).toHaveBeenCalledTimes(1);
    expect(sessions.getSnapshot()).not.toBe(before);

    // Nothing changed: no notification, and the same snapshot.
    const unchanged = sessions.getSnapshot();
    sessions.activate(key);
    expect(listener).toHaveBeenCalledTimes(1);
    expect(sessions.getSnapshot()).toBe(unchanged);

    unsubscribe();
    sessions.open("s-b");
    expect(listener).toHaveBeenCalledTimes(1);
  });

  it("closes the session of a closed terminal and shows a neighbour", () => {
    const { sessions, controllers, closed } = store();
    const first = sessions.open("s-a");
    const second = sessions.open("s-b");
    const third = sessions.open("s-c");

    sessions.close(third);
    expect(controllers[2]?.dispose).toHaveBeenCalledTimes(1);
    expect(closed).toEqual(["s-c"]);
    expect(sessions.getSnapshot()).toEqual({ keys: [first, second], activeKey: second });

    // Closing a terminal that is not on screen changes nothing on screen.
    sessions.close(first);
    expect(sessions.getSnapshot()).toEqual({ keys: [second], activeKey: second });
    expect(sessions.get(first)).toBeUndefined();

    sessions.close(second);
    sessions.close(second);
    expect(sessions.getSnapshot()).toEqual({ keys: [], activeKey: null });
    expect(controllers[1]?.dispose).toHaveBeenCalledTimes(1);
    expect(closed).toEqual(["s-c", "s-a", "s-b"]);
  });
});

describe("sessionToRestore", () => {
  it("prefers the session of the focused pane", () => {
    const sessions = [session("s-a"), session("s-b"), session("s-c")];
    const layout = {
      root: {
        type: "split" as const,
        id: 3,
        direction: "row" as const,
        ratio: 0.5,
        first: { type: "pane" as const, id: 1, session: "s-b" },
        second: { type: "pane" as const, id: 2, session: "s-c" },
      },
      focused: 2,
      nextId: 4,
    };

    expect(sessionToRestore(workspace(sessions, { layout }))?.id).toBe("s-c");
    expect(sessionToRestore(workspace(sessions, { layout: { ...layout, focused: 1 } }))?.id).toBe(
      "s-b",
    );
  });

  it("falls back to the first session, and to nothing in an empty workspace", () => {
    const sessions = [session("s-a"), session("s-b")];

    // The focused pane is empty, or shows a session that is gone.
    expect(sessionToRestore(workspace(sessions, { layout: singlePane(null) }))?.id).toBe("s-a");
    expect(sessionToRestore(workspace(sessions, { layout: singlePane("s-gone") }))?.id).toBe("s-a");
    expect(sessionToRestore(workspace())).toBeUndefined();
  });
});
