import { describe, expect, it, vi } from "vitest";

import type { Session } from "../lib/ipc";
import { renderApp } from "../test/fakeWorkspace";
import { session, singlePane, workspace } from "../test/workspaceFixtures";

/** How often each row described its state: once per render of the row. */
const described = vi.hoisted(() => new Map<string, number>());

vi.mock("../workspace/describe", async (importOriginal) => {
  const original = await importOriginal<typeof import("../workspace/describe")>();
  return {
    ...original,
    describeState: (each: Session) => {
      described.set(each.id, (described.get(each.id) ?? 0) + 1);
      return original.describeState(each);
    },
  };
});

const running = (id: string, overrides: Partial<Session> = {}) =>
  session(id, { state: { status: "running" }, terminalId: 1, ...overrides });

describe("the session tree", () => {
  it("renders again only the row whose session changed", async () => {
    const sessions = Array.from({ length: 100 }, (_, index) => running(`s-${String(index)}`));
    // Nothing on screen: the panes describe nothing.
    const view = await renderApp(workspace(sessions, { layout: singlePane(null) }));
    described.clear();

    view.update(running("s-42", { title: "vim notes.md", autoTitle: "vim notes.md" }));

    expect([...described.keys()]).toEqual(["s-42"]);
  });

  it("does not render rows again for a change that is not about them", async () => {
    const sessions = Array.from({ length: 20 }, (_, index) => running(`s-${String(index)}`));
    const view = await renderApp(workspace(sessions, { layout: singlePane(null) }));
    described.clear();

    view.push({ recentFolders: ["/elsewhere"] });
    view.push({ persistence: { mode: "failing", reason: "full", since: 1 } });

    expect(described.size).toBe(0);
  });
});
