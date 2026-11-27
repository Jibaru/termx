import { describe, expect, it } from "vitest";

import type { Layout, LayoutNode } from "../lib/ipc";
import {
  neighbourPane,
  paneOfSession,
  panesOf,
  sessionOfPane,
  visibleSessions,
} from "./layoutTree";

const pane = (id: number, session: string | null = null): LayoutNode => ({
  type: "pane",
  id,
  session,
});

//  +-----+-----+
//  |     |  2  |
//  |  1  +-----+
//  |     |  3  |
//  +-----+-----+
const tree: LayoutNode = {
  type: "split",
  id: 10,
  direction: "row",
  ratio: 0.5,
  first: pane(1, "s-a"),
  second: {
    type: "split",
    id: 11,
    direction: "column",
    ratio: 0.5,
    first: pane(2, "s-b"),
    second: pane(3),
  },
};
const layout: Layout = { root: tree, focused: 1, nextId: 12 };

describe("the layout tree", () => {
  it("lists the panes in reading order with where they are", () => {
    expect(panesOf(tree)).toEqual([
      { id: 1, session: "s-a", rect: { x: 0, y: 0, width: 0.5, height: 1 } },
      { id: 2, session: "s-b", rect: { x: 0.5, y: 0, width: 0.5, height: 0.5 } },
      { id: 3, session: null, rect: { x: 0.5, y: 0.5, width: 0.5, height: 0.5 } },
    ]);
  });

  it("finds what a pane shows, and where a session is", () => {
    expect(sessionOfPane(layout, 2)).toBe("s-b");
    expect(sessionOfPane(layout, 3)).toBeNull();
    expect(paneOfSession(layout, "s-a")).toBe(1);
    expect(paneOfSession(layout, "s-x")).toBeNull();
    expect(visibleSessions(layout)).toEqual(new Set(["s-a", "s-b"]));
  });

  it("finds the pane next to another in each direction", () => {
    expect(neighbourPane(tree, 1, "right")).toBe(2);
    expect(neighbourPane(tree, 2, "down")).toBe(3);
    expect(neighbourPane(tree, 3, "up")).toBe(2);
    expect(neighbourPane(tree, 3, "left")).toBe(1);
    expect(neighbourPane(tree, 1, "left")).toBeNull();
    expect(neighbourPane(tree, 2, "up")).toBeNull();
  });
});
