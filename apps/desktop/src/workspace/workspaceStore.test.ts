import { describe, expect, it } from "vitest";

import type { WorkspaceDelta } from "../lib/ipc";
import { fakeWorkspace } from "../test/fakeWorkspace";
import { recordIPC } from "../test/ipcRecorder";
import { deferred } from "../test/terminalHarness";
import { session, workspace } from "../test/workspaceFixtures";
import { WorkspaceStore } from "./workspaceStore";

const delta = (overrides: Partial<WorkspaceDelta>): WorkspaceDelta => ({
  revision: 10,
  sessions: [],
  removed: [],
  sessionOrder: null,
  projects: null,
  layout: null,
  recentFolders: null,
  persistence: null,
  ...overrides,
});

describe("WorkspaceStore", () => {
  it("subscribes once and keeps the snapshot", async () => {
    const fake = fakeWorkspace(workspace([session("s-1")]));
    const store = new WorkspaceStore(fake.backend);

    const [first, second] = await Promise.all([store.start(), store.start()]);

    expect(first).toBe(second);
    expect(fake.callsOf("subscribeWorkspace")).toHaveLength(1);
    expect(store.workspace?.sessions.map((each) => each.id)).toEqual(["s-1"]);
  });

  it("applies deltas as they arrive, without polling", async () => {
    const fake = fakeWorkspace(workspace([session("s-1")]));
    const store = new WorkspaceStore(fake.backend);
    await store.start();
    let changes = 0;
    store.subscribe(() => {
      changes += 1;
    });

    fake.push({ sessions: [session("s-1", { title: "Renamed", name: "Renamed" })] });
    fake.push({ sessions: [session("s-2")], sessionOrder: ["s-2", "s-1"] });

    expect(changes).toBe(2);
    expect(store.workspace?.sessions.map((each) => [each.id, each.title])).toEqual([
      ["s-2", "Shell"],
      ["s-1", "Renamed"],
    ]);
    expect(fake.calls.map((call) => call.command)).toEqual(["subscribeWorkspace"]);
  });

  it("applies the deltas that arrive before the snapshot on top of it", async () => {
    const snapshot = deferred<ReturnType<typeof workspace>>();
    let onDelta: ((d: WorkspaceDelta) => void) | undefined;
    const fake = fakeWorkspace(workspace());
    const store = new WorkspaceStore({
      ...fake.backend,
      subscribeWorkspace: (listener) => {
        onDelta = listener;
        return snapshot.promise;
      },
    });

    const started = store.start();
    onDelta?.(delta({ sessions: [session("s-early")] }));
    snapshot.resolve(workspace([session("s-1")]));
    await started;

    expect(store.workspace?.sessions.map((each) => each.id)).toEqual(["s-1", "s-early"]);
  });

  it("says why when it cannot subscribe, and tries again when asked", async () => {
    const ipc = recordIPC();
    const fake = fakeWorkspace(workspace());
    let fail = true;
    const store = new WorkspaceStore({
      ...fake.backend,
      subscribeWorkspace: (listener) =>
        fail ? Promise.reject(new Error("no window")) : fake.backend.subscribeWorkspace(listener),
    });

    expect(await store.start()).toBeNull();
    expect(store.getState()).toEqual({ status: "failed", message: "no window" });
    await expect.poll(() => ipc.logged().length).toBe(1);

    fail = false;
    expect(await store.start()).not.toBeNull();
    expect(store.getState().status).toBe("ready");
  });
});
