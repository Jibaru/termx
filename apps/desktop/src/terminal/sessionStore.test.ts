import { describe, expect, it, vi } from "vitest";

import { SessionStore } from "./sessionStore";
import type { TerminalController } from "./TerminalController";

/** A store whose terminals are stand-ins that only record their disposal. */
function store() {
  const controllers: { dispose: ReturnType<typeof vi.fn> }[] = [];
  const sessions = new SessionStore(() => {
    const controller = { dispose: vi.fn() };
    controllers.push(controller);
    return controller as unknown as TerminalController;
  });
  return { sessions, controllers };
}

describe("SessionStore", () => {
  it("starts empty", () => {
    const { sessions } = store();

    expect(sessions.getSnapshot()).toEqual({ keys: [], activeKey: null });
  });

  it("opens the first terminal once, however often it is asked", () => {
    const { sessions, controllers } = store();

    sessions.ensureOne();
    sessions.ensureOne();

    expect(controllers).toHaveLength(1);
    const { keys, activeKey } = sessions.getSnapshot();
    expect(keys).toHaveLength(1);
    expect(activeKey).toBe(keys[0]);
    expect(sessions.get(keys[0] ?? "")).toBe(controllers[0]);
  });

  it("keeps several terminals by key and shows the one opened last", () => {
    const { sessions, controllers } = store();

    const first = sessions.open();
    const second = sessions.open();

    expect(first).not.toBe(second);
    expect(sessions.getSnapshot()).toEqual({ keys: [first, second], activeKey: second });
    expect(sessions.get(first)).toBe(controllers[0]);
    expect(sessions.get(second)).toBe(controllers[1]);

    sessions.activate(first);
    expect(sessions.getSnapshot().activeKey).toBe(first);
    sessions.activate("unknown");
    expect(sessions.getSnapshot().activeKey).toBe(first);
  });

  it("notifies its subscribers with a new snapshot for every change", () => {
    const { sessions } = store();
    const listener = vi.fn();
    const before = sessions.getSnapshot();
    const unsubscribe = sessions.subscribe(listener);

    const key = sessions.open();
    expect(listener).toHaveBeenCalledTimes(1);
    expect(sessions.getSnapshot()).not.toBe(before);

    // Nothing changed: no notification, and the same snapshot.
    const unchanged = sessions.getSnapshot();
    sessions.activate(key);
    expect(listener).toHaveBeenCalledTimes(1);
    expect(sessions.getSnapshot()).toBe(unchanged);

    unsubscribe();
    sessions.open();
    expect(listener).toHaveBeenCalledTimes(1);
  });

  it("disposes of a closed terminal and shows a neighbour", () => {
    const { sessions, controllers } = store();
    const first = sessions.open();
    const second = sessions.open();
    const third = sessions.open();

    sessions.close(third);
    expect(controllers[2]?.dispose).toHaveBeenCalledTimes(1);
    expect(sessions.getSnapshot()).toEqual({ keys: [first, second], activeKey: second });

    // Closing a terminal that is not on screen changes nothing on screen.
    sessions.close(first);
    expect(sessions.getSnapshot()).toEqual({ keys: [second], activeKey: second });
    expect(sessions.get(first)).toBeUndefined();

    sessions.close(second);
    sessions.close(second);
    expect(sessions.getSnapshot()).toEqual({ keys: [], activeKey: null });
    expect(controllers[1]?.dispose).toHaveBeenCalledTimes(1);
  });
});
