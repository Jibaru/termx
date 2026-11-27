import type { Layout, LayoutNode, SessionId } from "../lib/ipc";

/** Reading the layout tree the Rust side sends. It is never changed here. */

export interface PaneInfo {
  id: number;
  session: SessionId | null;
  /** Where the pane is, as fractions of the main area. */
  rect: { x: number; y: number; width: number; height: number };
}

/** Every pane, in reading order, with where it is. */
export function panesOf(root: LayoutNode): PaneInfo[] {
  const panes: PaneInfo[] = [];
  const visit = (node: LayoutNode, rect: PaneInfo["rect"]) => {
    if (node.type === "pane") {
      panes.push({ id: node.id, session: node.session, rect });
      return;
    }
    if (node.direction === "row") {
      const first = rect.width * node.ratio;
      visit(node.first, { ...rect, width: first });
      visit(node.second, { ...rect, x: rect.x + first, width: rect.width - first });
    } else {
      const first = rect.height * node.ratio;
      visit(node.first, { ...rect, height: first });
      visit(node.second, { ...rect, y: rect.y + first, height: rect.height - first });
    }
  };
  visit(root, { x: 0, y: 0, width: 1, height: 1 });
  return panes;
}

/** The pane that shows `session`, if one does. */
export function paneOfSession(layout: Layout, session: SessionId): number | null {
  return panesOf(layout.root).find((pane) => pane.session === session)?.id ?? null;
}

/** What the pane `pane` shows. */
export function sessionOfPane(layout: Layout, pane: number): SessionId | null {
  return panesOf(layout.root).find((info) => info.id === pane)?.session ?? null;
}

/** The sessions shown in some pane. */
export function visibleSessions(layout: Layout): Set<SessionId> {
  const shown = new Set<SessionId>();
  for (const pane of panesOf(layout.root)) if (pane.session !== null) shown.add(pane.session);
  return shown;
}

export type Direction = "left" | "right" | "up" | "down";

const EPSILON = 1e-6;

/**
 * The pane next to `from` in `direction`: among the panes that touch its
 * side and overlap it along that side, the one closest to its centre.
 * `null` at the edge of the main area.
 */
export function neighbourPane(root: LayoutNode, from: number, direction: Direction): number | null {
  const panes = panesOf(root);
  const origin = panes.find((pane) => pane.id === from);
  if (!origin) return null;
  const o = origin.rect;
  const horizontal = direction === "left" || direction === "right";

  let best: { id: number; distance: number } | null = null;
  for (const pane of panes) {
    if (pane.id === from) continue;
    const r = pane.rect;
    const touches =
      direction === "left"
        ? Math.abs(r.x + r.width - o.x) < EPSILON
        : direction === "right"
          ? Math.abs(o.x + o.width - r.x) < EPSILON
          : direction === "up"
            ? Math.abs(r.y + r.height - o.y) < EPSILON
            : Math.abs(o.y + o.height - r.y) < EPSILON;
    if (!touches) continue;
    const overlap = horizontal
      ? Math.min(o.y + o.height, r.y + r.height) - Math.max(o.y, r.y)
      : Math.min(o.x + o.width, r.x + r.width) - Math.max(o.x, r.x);
    if (overlap <= EPSILON) continue;
    const distance = horizontal
      ? Math.abs(r.y + r.height / 2 - (o.y + o.height / 2))
      : Math.abs(r.x + r.width / 2 - (o.x + o.width / 2));
    if (!best || distance < best.distance) best = { id: pane.id, distance };
  }
  return best?.id ?? null;
}
