import {
  type DragEvent,
  type KeyboardEvent,
  type ReactNode,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import type { Session, SessionId, WorkspaceSnapshot } from "../lib/ipc";
import { isStringArray, readPref, writePref } from "../lib/prefs";
import { useShortcut, useUi, useWorkbench } from "../workspace/context";
import { isLive } from "../workspace/describe";
import { buildSidebar, dropIndex } from "../workspace/grouping";
import { sessionOfPane, visibleSessions } from "../workspace/layoutTree";
import { ChevronIcon, PlusIcon } from "./icons";
import { Menu, type MenuItem } from "./Menu";
import { groupElementId, rowElementId } from "./elementIds";
import { SessionRow } from "./SessionRow";

/** The key of the background section, among the keys of project groups. */
const BACKGROUND = "\u0000background";
const COLLAPSED_PREF = "collapsedGroups";

/** A focusable item of the tree: a group header or a session. */
type Item = { type: "group"; key: string } | { type: "session"; id: SessionId; group: string };

function itemKey(item: Item): string {
  return item.type === "group" ? `g:${item.key}` : `s:${item.id}`;
}

function elementIdOf(item: Item): string {
  return item.type === "group" ? groupElementId(item.key) : rowElementId(item.id);
}

type Drag =
  | { type: "session"; id: SessionId; siblings: SessionId[] }
  | { type: "project"; key: string; keys: string[] };

interface DropMark {
  target: string;
  position: "before" | "after";
}

/**
 * The sessions of the sidebar as an ARIA tree: project groups, then the
 * background commands.
 *
 * Keys: Up and Down move, Home and End jump, Left and Right collapse and
 * expand, Enter shows the session, Alt+Up and Alt+Down move the session (or
 * the group) within its list, F2 renames, the context-menu key or
 * Shift+F10 opens the menu. Sessions and groups can also be dragged.
 */
export function SessionTree({ workspace }: { workspace: WorkspaceSnapshot }) {
  const workbench = useWorkbench();
  const ui = useUi();
  const model = useMemo(() => buildSidebar(workspace), [workspace]);
  const [collapsed, setCollapsed] = useState<Set<string>>(
    () => new Set(readPref(COLLAPSED_PREF, [], isStringArray)),
  );
  const [cursor, setCursor] = useState<string | null>(null);
  const [menu, setMenu] = useState<{ session: SessionId; at: { x: number; y: number } } | null>(
    null,
  );
  const [drag, setDrag] = useState<Drag | null>(null);
  const [drop, setDrop] = useState<DropMark | null>(null);
  const tree = useRef<HTMLUListElement>(null);
  const focusAfterRender = useRef(false);

  const shortcuts = {
    rename: useShortcut("renameSession"),
    sibling: useShortcut("newSiblingSession"),
    close: useShortcut("closeSession"),
  };

  const active = sessionOfPane(workspace.layout, workspace.layout.focused);
  const visible = visibleSessions(workspace.layout);

  const sections = useMemo(() => {
    const list: { key: string; name: string; title: string; sessions: Session[] }[] =
      model.groups.map((group) => ({
        key: group.project.key,
        name: group.project.name,
        title: group.project.root,
        sessions: group.sessions,
      }));
    if (model.background.length > 0) {
      list.push({
        key: BACKGROUND,
        name: "Background",
        title: "Background commands",
        sessions: model.background,
      });
    }
    return list;
  }, [model]);

  const items = useMemo(() => {
    const list: Item[] = [];
    for (const section of sections) {
      list.push({ type: "group", key: section.key });
      if (collapsed.has(section.key)) continue;
      for (const session of section.sessions) {
        list.push({ type: "session", id: session.id, group: section.key });
      }
    }
    return list;
  }, [sections, collapsed]);

  // The item that takes Tab: the one last moved to, else the active session.
  const tabbable =
    items.find((item) => itemKey(item) === cursor) ??
    items.find((item) => item.type === "session" && item.id === active) ??
    items[0];

  useEffect(() => {
    // The name being edited keeps the keyboard.
    if (!focusAfterRender.current || !tabbable || ui.renaming !== null) return;
    focusAfterRender.current = false;
    document.getElementById(elementIdOf(tabbable))?.focus();
  });

  const toggle = (key: string, open?: boolean) => {
    setCollapsed((previous) => {
      const next = new Set(previous);
      const shouldOpen = open ?? next.has(key);
      if (shouldOpen) next.delete(key);
      else next.add(key);
      writePref(COLLAPSED_PREF, [...next]);
      return next;
    });
  };

  const moveTo = (item: Item | undefined) => {
    if (!item) return;
    setCursor(itemKey(item));
    focusAfterRender.current = true;
    document.getElementById(elementIdOf(item))?.focus();
  };

  const openMenu = (session: SessionId, at: { x: number; y: number }) => {
    setMenu({ session, at });
  };

  const onKeyDown = (event: KeyboardEvent) => {
    // The item the key was pressed on: the focused one.
    const element = (event.target as Element).closest('[role="treeitem"]');
    const index = items.findIndex((item) => elementIdOf(item) === element?.id);
    const current = items[index];
    if (!current) return;

    if (event.altKey && (event.key === "ArrowUp" || event.key === "ArrowDown")) {
      event.preventDefault();
      const step = event.key === "ArrowUp" ? -1 : 1;
      focusAfterRender.current = true;
      if (current.type === "session") void workbench.moveSession(current.id, step);
      else if (current.key !== BACKGROUND) void workbench.moveProject(current.key, step);
      return;
    }
    if (event.altKey || event.ctrlKey || event.metaKey) return;

    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        moveTo(items[Math.min(index + 1, items.length - 1)]);
        break;
      case "ArrowUp":
        event.preventDefault();
        moveTo(items[Math.max(index - 1, 0)]);
        break;
      case "Home":
        event.preventDefault();
        moveTo(items[0]);
        break;
      case "End":
        event.preventDefault();
        moveTo(items.at(-1));
        break;
      case "ArrowRight":
        event.preventDefault();
        if (current.type === "group") {
          if (collapsed.has(current.key)) toggle(current.key, true);
          else moveTo(items[index + 1]);
        }
        break;
      case "ArrowLeft":
        event.preventDefault();
        if (current.type === "session") {
          moveTo(items.find((item) => item.type === "group" && item.key === current.group));
        } else if (!collapsed.has(current.key)) {
          toggle(current.key, false);
        }
        break;
      case "Enter":
      case " ":
        event.preventDefault();
        if (current.type === "group") toggle(current.key);
        else void workbench.activate(current.id);
        break;
      case "ContextMenu":
      case "F10":
        if (event.key === "F10" && !event.shiftKey) return;
        if (current.type === "session") {
          event.preventDefault();
          const rect = document.getElementById(elementIdOf(current))?.getBoundingClientRect();
          openMenu(current.id, { x: (rect?.left ?? 0) + 24, y: rect?.bottom ?? 0 });
        }
        break;
    }
  };

  // --- Dragging -------------------------------------------------------------

  const startSessionDrag = (event: DragEvent, id: SessionId, siblings: SessionId[]) => {
    event.stopPropagation();
    event.dataTransfer.effectAllowed = "move";
    // Some engines only start a drag that carries data.
    event.dataTransfer.setData("text/plain", id);
    setDrag({ type: "session", id, siblings });
    workbench.setDragging(id);
  };

  const startProjectDrag = (event: DragEvent, key: string) => {
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", key);
    setDrag({ type: "project", key, keys: model.groups.map((group) => group.project.key) });
  };

  const endDrag = () => {
    setDrag(null);
    setDrop(null);
    workbench.setDragging(null);
  };

  const position = (event: DragEvent): "before" | "after" => {
    const rect = event.currentTarget.getBoundingClientRect();
    return event.clientY < rect.top + rect.height / 2 ? "before" : "after";
  };

  const overSession = (event: DragEvent, target: SessionId) => {
    if (drag?.type !== "session" || !drag.siblings.includes(target)) return;
    event.preventDefault();
    event.stopPropagation();
    event.dataTransfer.dropEffect = "move";
    if (target === drag.id) {
      setDrop(null);
      return;
    }
    setDrop({ target: `s:${target}`, position: position(event) });
  };

  const dropOnSession = (event: DragEvent, target: SessionId) => {
    if (drag?.type !== "session" || !drag.siblings.includes(target)) return;
    event.preventDefault();
    event.stopPropagation();
    if (target !== drag.id) {
      const index = dropIndex(drag.siblings, drag.id, target, position(event));
      void workbench.placeSession(drag.id, drag.siblings, index);
    }
    endDrag();
  };

  const overGroup = (event: DragEvent, key: string) => {
    if (drag?.type !== "project" || !drag.keys.includes(key)) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = "move";
    setDrop(key === drag.key ? null : { target: `g:${key}`, position: position(event) });
  };

  const dropOnGroup = (event: DragEvent, key: string) => {
    if (drag?.type !== "project" || !drag.keys.includes(key)) return;
    event.preventDefault();
    if (key !== drag.key) {
      const index = dropIndex(drag.keys, drag.key, key, position(event));
      void workbench.placeProject(drag.key, drag.keys, index);
    }
    endDrag();
  };

  // --- Menu -------------------------------------------------------------------

  const menuItems = (id: SessionId): MenuItem[] => {
    const session = workspace.sessions.find((candidate) => candidate.id === id);
    if (!session) return [];
    const items: MenuItem[] = [
      {
        label: "Show in focused pane",
        onSelect: () => void workbench.showInFocusedPane(id),
        testId: "menu-show-in-focused-pane",
      },
      {
        label: "Open in a split on the right",
        onSelect: () =>
          void workbench.splitWith(workspace.layout.focused, "row", "after", session.id),
        testId: "menu-open-in-split",
      },
      {
        label: "Rename",
        shortcut: shortcuts.rename,
        onSelect: () => {
          workbench.startRenaming(id);
        },
        testId: "menu-rename",
      },
      {
        label: "New Claude session in this project",
        shortcut: shortcuts.sibling,
        onSelect: () => void workbench.createSibling(id),
        testId: "menu-new-sibling",
      },
    ];
    if (session.state.status === "dormant") {
      items.push({
        label: "Resume",
        onSelect: () => {
          workbench.resume(id);
        },
        testId: "menu-resume",
      });
    } else if (session.state.status === "exited") {
      items.push({
        label: session.kind === "claude" ? "Resume" : "Restart",
        onSelect: () => {
          if (session.kind === "claude") workbench.resume(id);
          else workbench.restart(id);
        },
        testId: "menu-restart",
      });
    }
    if (isLive(session)) {
      items.push({
        label: "Restart…",
        onSelect: () => {
          workbench.restart(id);
        },
        testId: "menu-restart-running",
      });
      items.push({
        label: "Stop",
        onSelect: () => void workbench.stop(id),
        testId: "menu-stop",
      });
    }
    items.push(
      {
        label: "Move up",
        shortcut: "Alt+↑",
        onSelect: () => void workbench.moveSession(id, -1),
        testId: "menu-move-up",
      },
      {
        label: "Move down",
        shortcut: "Alt+↓",
        onSelect: () => void workbench.moveSession(id, 1),
        testId: "menu-move-down",
      },
      {
        label: "Close session",
        shortcut: shortcuts.close,
        danger: true,
        onSelect: () => void workbench.requestClose(id),
        testId: "menu-close",
      },
    );
    return items;
  };

  // --- Rendering ----------------------------------------------------------------

  const renderRow = (session: Session, siblings: SessionId[], showProject: boolean): ReactNode => {
    const key = `s:${session.id}`;
    return (
      <SessionRow
        key={session.id}
        session={session}
        active={session.id === active}
        visible={visible.has(session.id)}
        tabbable={tabbable !== undefined && itemKey(tabbable) === key}
        renaming={ui.renaming === session.id}
        dropAt={drop?.target === key ? drop.position : null}
        showProject={showProject}
        onActivate={() => {
          setCursor(key);
          void workbench.activate(session.id);
        }}
        onStartRename={() => {
          workbench.startRenaming(session.id);
        }}
        onRename={(name) => {
          focusAfterRender.current = true;
          void workbench.rename(session.id, name);
        }}
        onCancelRename={() => {
          focusAfterRender.current = true;
          workbench.stopRenaming();
        }}
        onResume={() => {
          workbench.resume(session.id);
        }}
        onContextMenu={(event) => {
          event.preventDefault();
          event.stopPropagation();
          setCursor(key);
          openMenu(session.id, { x: event.clientX, y: event.clientY });
        }}
        onFocus={() => {
          setCursor(key);
        }}
        onDragStart={(event) => {
          startSessionDrag(event, session.id, siblings);
        }}
        onDragOver={(event) => {
          overSession(event, session.id);
        }}
        onDrop={(event) => {
          dropOnSession(event, session.id);
        }}
        onDragEnd={endDrag}
      />
    );
  };

  return (
    <>
      <ul
        ref={tree}
        role="tree"
        aria-label="Sessions"
        className="session-tree"
        data-testid="session-tree"
        onKeyDown={onKeyDown}
      >
        {sections.map((section) => {
          const open = !collapsed.has(section.key);
          const key = `g:${section.key}`;
          const siblings = section.sessions.map((session) => session.id);
          const isBackground = section.key === BACKGROUND;
          const first = section.sessions[0];
          return (
            <li
              key={section.key}
              id={groupElementId(section.key)}
              role="treeitem"
              aria-level={1}
              aria-expanded={open}
              aria-label={`${section.name}, ${String(section.sessions.length)} session${
                section.sessions.length === 1 ? "" : "s"
              }`}
              tabIndex={tabbable !== undefined && itemKey(tabbable) === key ? 0 : -1}
              className="session-group"
              data-testid={isBackground ? "background-group" : "project-group"}
              data-project-key={isBackground ? undefined : section.key}
              data-drop={drop?.target === key ? drop.position : undefined}
              onFocus={(event) => {
                if (event.target === event.currentTarget) setCursor(key);
              }}
            >
              <div
                className="group-header"
                draggable={!isBackground}
                onClick={() => {
                  setCursor(key);
                  toggle(section.key);
                }}
                onDragStart={(event) => {
                  if (!isBackground) startProjectDrag(event, section.key);
                }}
                onDragOver={(event) => {
                  overGroup(event, section.key);
                }}
                onDrop={(event) => {
                  dropOnGroup(event, section.key);
                }}
                onDragEnd={endDrag}
              >
                <ChevronIcon className="group-chevron" />
                <span className="group-name" title={section.title}>
                  {section.name}
                </span>
                <span className="group-count" aria-hidden="true">
                  {section.sessions.length}
                </span>
                {!isBackground && first && (
                  <button
                    type="button"
                    className="icon-button group-add"
                    tabIndex={-1}
                    aria-label={`New Claude session in ${section.name}`}
                    title={`New Claude session in ${section.name}`}
                    onClick={(event) => {
                      event.stopPropagation();
                      void workbench.createSibling(first.id);
                    }}
                  >
                    <PlusIcon />
                  </button>
                )}
              </div>
              {open && (
                <ul role="group" className="session-group-rows">
                  {section.sessions.map((session) => renderRow(session, siblings, isBackground))}
                </ul>
              )}
            </li>
          );
        })}
      </ul>
      {menu && (
        <Menu
          label="Session"
          items={menuItems(menu.session)}
          at={menu.at}
          onClose={() => {
            setMenu(null);
            focusAfterRender.current = true;
          }}
        />
      )}
    </>
  );
}
