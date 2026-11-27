import { type KeyboardEvent, type PointerEvent, useRef } from "react";

import { clampSidebarWidth, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH } from "../lib/sidebarWidth";

const KEY_STEP = 16;

interface SidebarResizerProps {
  width: number;
  /** While dragging. */
  onResize: (width: number) => void;
  /** Once the width is settled, to remember it. */
  onCommit: (width: number) => void;
}

/** The edge of the sidebar: drag it, or use the arrow keys, to resize. */
export function SidebarResizer({ width, onResize, onCommit }: SidebarResizerProps) {
  const start = useRef<{ x: number; width: number } | null>(null);
  const last = useRef(width);

  const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    event.preventDefault();
    start.current = { x: event.clientX, width };
    last.current = width;
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const onPointerMove = (event: PointerEvent<HTMLDivElement>) => {
    if (!start.current) return;
    last.current = clampSidebarWidth(start.current.width + event.clientX - start.current.x);
    onResize(last.current);
  };

  const onPointerUp = (event: PointerEvent<HTMLDivElement>) => {
    if (!start.current) return;
    start.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    onCommit(last.current);
  };

  const onKeyDown = (event: KeyboardEvent) => {
    let next: number | null = null;
    if (event.key === "ArrowLeft") next = width - KEY_STEP;
    else if (event.key === "ArrowRight") next = width + KEY_STEP;
    if (next === null) return;
    event.preventDefault();
    onCommit(clampSidebarWidth(next));
  };

  return (
    <div
      role="separator"
      tabIndex={0}
      aria-orientation="vertical"
      aria-label="Resize the sidebar"
      aria-valuemin={SIDEBAR_MIN_WIDTH}
      aria-valuemax={SIDEBAR_MAX_WIDTH}
      aria-valuenow={width}
      className="sidebar-resizer"
      data-testid="sidebar-resizer"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onKeyDown={onKeyDown}
    />
  );
}
