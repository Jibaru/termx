import { describe, expect, it } from "vitest";

import { fakeTerminals } from "../test/fakeWorkspace";
import { session, singlePane, workspace } from "../test/workspaceFixtures";
import { autoStartFor, sessionToRestore } from "./sessionStore";

describe("SessionStore", () => {
  it("starts empty", () => {
    const { store } = fakeTerminals();

    expect(store.getSnapshot()).toEqual({ keys: [], activeKey: null });
  });

  it("makes the terminal of a session once, the first time it is asked for", () => {
    const { store, controllers } = fakeTerminals();
    const shell = session("s-1");

    const first = store.controllerFor(shell);
    const again = store.controllerFor(shell);

    expect(first).toBe(again);
    expect(controllers).toHaveLength(1);
    expect(store.get("s-1")).toBe(first);
    expect(store.getSnapshot().keys).toEqual(["s-1"]);
  });

  it("starts a new session at once and leaves a restored one waiting for the user", () => {
    expect(autoStartFor(session("s", { state: { status: "starting" } }))).toBe("fresh");
    expect(autoStartFor(session("s", { state: { status: "dormant" } }))).toBe("dormant");
    // A session that runs without a terminal here is not restarted behind
    // the user's back (the page was reloaded).
    expect(autoStartFor(session("s", { state: { status: "running" } }))).toBe("dormant");
    expect(
      autoStartFor(
        session("s", { state: { status: "exited", code: 0, signal: null, failure: null } }),
      ),
    ).toBe("dormant");
  });

  it("resumes a session that is not on screen, at the size of the focused terminal", () => {
    const { store, of } = fakeTerminals();
    const shown = session("s-shown", { state: { status: "running" } });
    store.controllerFor(shown);
    store.setActive("s-shown");

    store.resume(session("s-hidden", { state: { status: "dormant" } }));

    expect(of("s-hidden")?.startDetached).toHaveBeenCalledExactlyOnceWith("resume");
    // Resuming again while it starts does nothing.
    store.resume(session("s-hidden", { state: { status: "dormant" } }));
    expect(of("s-hidden")?.startDetached).toHaveBeenCalledTimes(1);
  });

  it("lets go of the terminals of sessions that left the workspace", () => {
    const { store, of } = fakeTerminals();
    store.controllerFor(session("s-1"));
    store.controllerFor(session("s-2"));
    store.setActive("s-2");

    store.retain(new Set(["s-1"]));

    expect(of("s-2")?.dispose).toHaveBeenCalledTimes(1);
    expect(of("s-1")?.dispose).not.toHaveBeenCalled();
    expect(store.getSnapshot()).toEqual({ keys: ["s-1"], activeKey: null });
  });

  it("names the session of the focused pane as the active one, for the end-to-end hook", () => {
    const { store } = fakeTerminals();
    store.controllerFor(session("s-1"));

    store.setActive("s-1");

    expect(store.getSnapshot().activeKey).toBe("s-1");
  });
});

describe("sessionToRestore", () => {
  it("is the session of the focused pane, else the first one", () => {
    const sessions = [session("s-first"), session("s-shown")];
    expect(sessionToRestore(workspace(sessions, { layout: singlePane("s-shown") }))?.id).toBe(
      "s-shown",
    );
    expect(sessionToRestore(workspace(sessions, { layout: singlePane(null) }))?.id).toBe("s-first");
    expect(sessionToRestore(workspace([]))).toBeUndefined();
  });
});
