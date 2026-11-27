import { type KeyboardEvent, useEffect, useRef } from "react";

export interface MenuItem {
  label: string;
  /** The shortcut, written for the user. */
  shortcut?: string | null;
  onSelect: () => void;
  danger?: boolean;
  testId?: string;
}

interface MenuProps {
  label: string;
  items: MenuItem[];
  /** Where the menu opens, in window coordinates. */
  at: { x: number; y: number };
  onClose: () => void;
}

/**
 * A small context menu: opens with its first item focused, moves with the
 * arrow keys, closes with Escape, a click elsewhere or a choice.
 */
export function Menu({ label, items, at, onClose }: MenuProps) {
  const menu = useRef<HTMLDivElement>(null);
  const close = useRef(onClose);

  useEffect(() => {
    close.current = onClose;
  });

  useEffect(() => {
    menu.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
    const onPointer = (event: PointerEvent) => {
      if (!menu.current?.contains(event.target as Node)) close.current();
    };
    window.addEventListener("pointerdown", onPointer, true);
    return () => {
      window.removeEventListener("pointerdown", onPointer, true);
    };
  }, []);

  const move = (event: KeyboardEvent, step: number) => {
    event.preventDefault();
    const entries = [...(menu.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])];
    const index = entries.indexOf(document.activeElement as HTMLElement);
    entries[(index + step + entries.length) % entries.length]?.focus();
  };

  return (
    <div
      ref={menu}
      role="menu"
      aria-label={label}
      className="menu"
      data-testid="context-menu"
      style={{ left: at.x, top: at.y }}
      onKeyDown={(event) => {
        event.stopPropagation();
        if (event.key === "ArrowDown") move(event, 1);
        else if (event.key === "ArrowUp") move(event, -1);
        else if (event.key === "Escape" || event.key === "Tab") {
          event.preventDefault();
          onClose();
        }
      }}
    >
      {items.map((item) => (
        <button
          key={item.label}
          type="button"
          role="menuitem"
          tabIndex={-1}
          className="menu-item"
          data-danger={item.danger ? true : undefined}
          data-testid={item.testId}
          onClick={() => {
            onClose();
            item.onSelect();
          }}
        >
          <span>{item.label}</span>
          {item.shortcut && <kbd className="menu-shortcut">{item.shortcut}</kbd>}
        </button>
      ))}
    </div>
  );
}
