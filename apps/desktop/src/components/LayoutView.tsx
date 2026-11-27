import {
  type CSSProperties,
  type KeyboardEvent,
  type PointerEvent,
  useEffect,
  useRef,
  useState,
} from "react";

import type { LayoutNode, WorkspaceSnapshot } from "../lib/ipc";
import { useWorkbench } from "../workspace/context";
import { PaneView } from "./PaneView";

/** The share of a split one key press of an arrow moves its divider by. */
export const DIVIDER_KEY_STEP = 0.05;
const MIN_RATIO = 0.1;
const MAX_RATIO = 0.9;

function clamp(ratio: number): number {
  return Math.min(MAX_RATIO, Math.max(MIN_RATIO, ratio));
}

interface LayoutViewProps {
  node: LayoutNode;
  workspace: WorkspaceSnapshot;
}

/**
 * The main area, drawn from the layout tree of the workspace: a pane, or a
 * split of two nodes side by side (`row`) or stacked (`column`).
 */
export function LayoutView({ node, workspace }: LayoutViewProps) {
  if (node.type === "pane") {
    return (
      <PaneView
        pane={node.id}
        sessionId={node.session}
        workspace={workspace}
        focused={workspace.layout.focused === node.id}
        alone={workspace.layout.root.type === "pane"}
      />
    );
  }
  return <SplitView node={node} workspace={workspace} />;
}

function SplitView({
  node,
  workspace,
}: {
  node: Extract<LayoutNode, { type: "split" }>;
  workspace: WorkspaceSnapshot;
}) {
  const workbench = useWorkbench();
  const container = useRef<HTMLDivElement>(null);
  // While a divider is dragged, it is drawn where the pointer is; the ratio
  // the Rust side keeps follows once a frame.
  const [dragRatio, setDragRatio] = useState<number | null>(null);
  const dragging = useRef(false);
  const pending = useRef<number | null>(null);
  const frame = useRef<number | null>(null);

  const ratio = dragRatio ?? node.ratio;
  const row = node.direction === "row";

  useEffect(() => {
    // The ratio the Rust side set has arrived: it is the one to draw.
    if (!dragging.current) setDragRatio(null);
  }, [node.ratio]);

  useEffect(
    () => () => {
      if (frame.current !== null) cancelAnimationFrame(frame.current);
    },
    [],
  );

  const send = (value: number) => {
    pending.current = value;
    if (frame.current !== null) return;
    frame.current = requestAnimationFrame(() => {
      frame.current = null;
      const next = pending.current;
      pending.current = null;
      if (next !== null) void workbench.resizeSplit(node.id, next);
    });
  };

  const ratioAt = (event: PointerEvent): number | null => {
    const rect = container.current?.getBoundingClientRect();
    if (!rect) return null;
    const size = row ? rect.width : rect.height;
    if (size <= 0) return null;
    return clamp((row ? event.clientX - rect.left : event.clientY - rect.top) / size);
  };

  const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    event.preventDefault();
    dragging.current = true;
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const onPointerMove = (event: PointerEvent<HTMLDivElement>) => {
    if (!dragging.current) return;
    const next = ratioAt(event);
    if (next === null) return;
    setDragRatio(next);
    send(next);
  };

  const onPointerUp = (event: PointerEvent<HTMLDivElement>) => {
    if (!dragging.current) return;
    dragging.current = false;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    const next = ratioAt(event);
    if (next !== null) {
      setDragRatio(next);
      void workbench.resizeSplit(node.id, next);
    }
  };

  const onKeyDown = (event: KeyboardEvent) => {
    const less = row ? "ArrowLeft" : "ArrowUp";
    const more = row ? "ArrowRight" : "ArrowDown";
    let next: number | null = null;
    if (event.key === less) next = ratio - DIVIDER_KEY_STEP;
    else if (event.key === more) next = ratio + DIVIDER_KEY_STEP;
    else if (event.key === "Home") next = MIN_RATIO;
    else if (event.key === "End") next = MAX_RATIO;
    if (next === null) return;
    event.preventDefault();
    const value = clamp(Math.round(next * 100) / 100);
    setDragRatio(value);
    void workbench.resizeSplit(node.id, value);
  };

  const firstStyle: CSSProperties = { flexBasis: `${String(ratio * 100)}%` };

  return (
    <div
      ref={container}
      className="split"
      data-testid="split"
      data-split-id={node.id}
      data-direction={node.direction}
    >
      <div className="split-child" style={firstStyle}>
        <LayoutView node={node.first} workspace={workspace} />
      </div>
      <div
        role="separator"
        tabIndex={0}
        aria-orientation={row ? "vertical" : "horizontal"}
        aria-valuemin={MIN_RATIO * 100}
        aria-valuemax={MAX_RATIO * 100}
        aria-valuenow={Math.round(ratio * 100)}
        aria-label={row ? "Resize the panes side by side" : "Resize the stacked panes"}
        className="split-divider"
        data-testid="split-divider"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
        onKeyDown={onKeyDown}
      />
      <div className="split-child split-child-rest">
        <LayoutView node={node.second} workspace={workspace} />
      </div>
    </div>
  );
}
