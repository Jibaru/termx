import { describe, expect, it } from "vitest";

import type { Project } from "../lib/ipc";
import { session, workspace } from "../test/workspaceFixtures";
import { beforeFor, buildSidebar, dropIndex } from "./grouping";

const app: Project = { key: "/work/app", name: "app", root: "/work/app", kind: "git" };
const api: Project = { key: "/work/api", name: "api", root: "/work/api", kind: "git" };

describe("the sidebar model", () => {
  it("groups sessions by project, in the order of the projects", () => {
    const snapshot = workspace([
      session("s-1", { project: app }),
      session("s-2", { project: api }),
      session("s-3", { project: app }),
    ]);
    snapshot.projects = [api, app];

    const model = buildSidebar(snapshot);

    expect(model.groups.map((group) => group.project.name)).toEqual(["api", "app"]);
    expect(model.groups[1]?.sessions.map((each) => each.id)).toEqual(["s-1", "s-3"]);
    expect(model.order).toEqual(["s-2", "s-1", "s-3"]);
  });

  it("puts a worktree with its main repository, as the Rust side says", () => {
    const model = buildSidebar(
      workspace([
        session("s-main", { project: app }),
        session("s-tree", {
          project: app,
          cwd: "/work/app-feature",
          git: { branch: "feature", commit: null, worktree: "app-feature" },
        }),
      ]),
    );

    expect(model.groups).toHaveLength(1);
    expect(model.groups[0]?.sessions.map((each) => each.id)).toEqual(["s-main", "s-tree"]);
  });

  it("keeps background commands in a section of their own, last", () => {
    const model = buildSidebar(
      workspace([
        session("s-bg", { kind: "background", project: app, command: "npm run dev" }),
        session("s-1", { project: app }),
      ]),
    );

    expect(model.groups[0]?.sessions.map((each) => each.id)).toEqual(["s-1"]);
    expect(model.background.map((each) => each.id)).toEqual(["s-bg"]);
    expect(model.order).toEqual(["s-1", "s-bg"]);
  });

  it("does not show a project without sessions, and still shows one the list misses", () => {
    const snapshot = workspace([session("s-1", { project: api })]);
    snapshot.projects = [app];

    expect(buildSidebar(snapshot).groups.map((group) => group.project.key)).toEqual(["/work/api"]);
  });
});

describe("moving within a list", () => {
  const list = ["a", "b", "c", "d"];

  it("names the item to go before", () => {
    expect(beforeFor(list, "d", 0)).toBe("a");
    expect(beforeFor(list, "a", 2)).toBe("d");
  });

  it("goes to the end with null", () => {
    expect(beforeFor(list, "a", 3)).toBeNull();
  });

  it("does nothing when it stays where it is", () => {
    expect(beforeFor(list, "b", 1)).toBeUndefined();
    expect(beforeFor(list, "x", 1)).toBeUndefined();
  });

  it("drops before or after the target", () => {
    expect(beforeFor(list, "a", dropIndex(list, "a", "c", "after"))).toBe("d");
    expect(beforeFor(list, "d", dropIndex(list, "d", "b", "before"))).toBe("b");
    expect(beforeFor(list, "a", dropIndex(list, "a", "d", "after"))).toBeNull();
  });
});
