import { type CSSProperties, useCallback, useEffect, useMemo, useRef, useState } from "react";

import { Notices, PersistenceBanner, RestoreBanner } from "./components/Banners";
import {
  ConfirmCloseDialog,
  ConfirmQuitDialog,
  ConfirmRestartDialog,
} from "./components/ConfirmCloseDialog";
import { PlusIcon, SidebarIcon } from "./icons/glyphs";
import { LayoutView } from "./components/LayoutView";
import { NewSessionDialog } from "./components/NewSessionDialog";
import { Sidebar } from "./components/Sidebar";
import { SidebarResizer } from "./components/SidebarResizer";
import {
  type Binding,
  COMMAND_LABELS,
  type CommandId,
  resolveBindings,
  shortcutOf,
} from "./keybindings/keybindings";
import { useKeybindings } from "./keybindings/useKeybindings";
import { detectPlatform, type Platform } from "./lib/platform";
import { isBoolean, isNumber, readPref, writePref } from "./lib/prefs";
import { clampSidebarWidth, SIDEBAR_DEFAULT_WIDTH } from "./lib/sidebarWidth";
import {
  useUi,
  useWorkbenchContext,
  useWorkspaceState,
  WorkbenchContext,
  type WorkbenchContextValue,
  withShortcut,
} from "./workspace/context";
import { defaultWorkbench } from "./workspace/instance";
import { sessionOfPane } from "./workspace/layoutTree";
import type { Workbench } from "./workspace/workbench";

interface AppProps {
  /** What the window acts on. Tests pass their own. */
  workbench?: Workbench;
  platform?: Platform;
  /** The key bindings; the defaults of the platform when omitted. */
  bindings?: readonly Binding[];
}

const WIDTH_PREF = "sidebarWidth";
const VISIBLE_PREF = "sidebarVisible";

/**
 * The window: the session sidebar on the left, and the main area split
 * into panes, with the banners that say what the user should know.
 */
export function App({ workbench = defaultWorkbench, platform, bindings }: AppProps) {
  const resolvedPlatform = useMemo(() => platform ?? detectPlatform(), [platform]);
  const resolvedBindings = useMemo(
    () => bindings ?? resolveBindings(resolvedPlatform),
    [bindings, resolvedPlatform],
  );
  const context = useMemo<WorkbenchContextValue>(
    () => ({ workbench, platform: resolvedPlatform, bindings: resolvedBindings }),
    [workbench, resolvedPlatform, resolvedBindings],
  );

  return (
    <WorkbenchContext value={context}>
      <Window />
    </WorkbenchContext>
  );
}

function Window() {
  const { workbench, bindings, platform } = useWorkbenchContext();
  const state = useWorkspaceState();
  const ui = useUi();
  const workspace = state.status === "ready" ? state.workspace : null;

  const [sidebarVisible, setSidebarVisible] = useState(() =>
    readPref(VISIBLE_PREF, true, isBoolean),
  );
  const [sidebarWidth, setSidebarWidth] = useState(() =>
    clampSidebarWidth(readPref(WIDTH_PREF, SIDEBAR_DEFAULT_WIDTH, isNumber)),
  );

  useEffect(() => {
    void workbench.bootstrap();
  }, [workbench]);

  // The terminals follow the workspace: those of closed sessions go, and
  // the end-to-end hook and the size of new terminals follow the focus.
  useEffect(() => {
    if (!workspace) return;
    workbench.terminals.retain(new Set(workspace.sessions.map((session) => session.id)));
    workbench.terminals.setActive(sessionOfPane(workspace.layout, workspace.layout.focused));
  }, [workspace, workbench]);

  // Branches may have changed while the window was in the background.
  useEffect(() => {
    const onFocus = () => {
      workbench.refreshGit();
    };
    window.addEventListener("focus", onFocus);
    return () => {
      window.removeEventListener("focus", onFocus);
    };
  }, [workbench]);

  const showSidebar = useCallback((visible: boolean) => {
    setSidebarVisible(visible);
    writePref(VISIBLE_PREF, visible);
  }, []);

  const focusSidebar = useCallback(() => {
    showSidebar(true);
    // Once the sidebar is on screen.
    requestAnimationFrame(() => {
      document.querySelector<HTMLElement>('.session-tree [tabindex="0"]')?.focus();
    });
  }, [showSidebar]);

  const run = useCallback(
    (command: CommandId, target: EventTarget | null) => {
      switch (command) {
        case "newSession":
          workbench.openNewSessionDialog();
          return;
        case "newSiblingSession":
          void workbench.createSibling();
          return;
        case "closeSession":
          void workbench.requestClose();
          return;
        case "splitRight":
          void workbench.split("row");
          return;
        case "splitDown":
          void workbench.split("column");
          return;
        case "closePane":
          void workbench.closePane();
          return;
        case "focusPaneLeft":
          void workbench.focusDirection("left");
          return;
        case "focusPaneRight":
          void workbench.focusDirection("right");
          return;
        case "focusPaneUp":
          void workbench.focusDirection("up");
          return;
        case "focusPaneDown":
          void workbench.focusDirection("down");
          return;
        case "nextSession":
          void workbench.cycle(1);
          return;
        case "previousSession":
          void workbench.cycle(-1);
          return;
        case "toggleSidebar":
          showSidebar(!sidebarVisible);
          return;
        case "focusSidebar":
          focusSidebar();
          return;
        case "renameSession": {
          const row =
            target instanceof Element
              ? target.closest<HTMLElement>("[data-session-id]")?.dataset.sessionId
              : undefined;
          const id = row ?? workbench.focusedSession()?.id;
          if (id === undefined) return;
          showSidebar(true);
          workbench.startRenaming(id);
          return;
        }
        default: {
          const n = Number(/^jumpToSession(\d)$/.exec(command)?.[1]);
          if (n > 0) void workbench.jumpTo(n);
        }
      }
    },
    [workbench, showSidebar, sidebarVisible, focusSidebar],
  );

  useKeybindings(bindings, run, ui.dialog.kind === "none");

  // The menu bar of macOS, and the confirmation before quitting.
  const latestRun = useRef({ run, enabled: ui.dialog.kind === "none" });
  useEffect(() => {
    latestRun.current = { run, enabled: ui.dialog.kind === "none" };
  });
  useEffect(() => {
    const backend = workbench.workspace.backend;
    const stops = [
      backend.onMenuCommand((command) => {
        if (!latestRun.current.enabled || !(command in COMMAND_LABELS)) return;
        latestRun.current.run(command as CommandId, document.activeElement);
      }),
      backend.onQuitRequested(workbench.requestQuit),
    ];
    return () => {
      for (const stop of stops) {
        stop.then((unlisten) => {
          unlisten();
        }, ignoreFailure);
      }
    };
  }, [workbench]);

  const shellStyle = {
    "--sidebar-width": `${String(sidebarWidth)}px`,
  } as CSSProperties;
  const toggleLabel = withShortcut(
    "Show the sidebar",
    shortcutOf(bindings, "toggleSidebar", platform),
  );
  const newLabel = withShortcut("New session", shortcutOf(bindings, "newSession", platform));

  return (
    <div
      className="app-shell"
      data-sidebar={sidebarVisible ? "visible" : "hidden"}
      style={shellStyle}
    >
      {sidebarVisible ? (
        <>
          <Sidebar
            onHide={() => {
              showSidebar(false);
            }}
          />
          <SidebarResizer
            width={sidebarWidth}
            onResize={setSidebarWidth}
            onCommit={(width) => {
              setSidebarWidth(width);
              writePref(WIDTH_PREF, width);
            }}
          />
        </>
      ) : (
        <nav className="sidebar-rail" aria-label="Sidebar">
          <button
            type="button"
            className="icon-button"
            data-testid="show-sidebar"
            aria-label={toggleLabel}
            title={toggleLabel}
            onClick={() => {
              showSidebar(true);
            }}
          >
            <SidebarIcon />
          </button>
          <button
            type="button"
            className="icon-button"
            aria-label={newLabel}
            title={newLabel}
            onClick={workbench.openNewSessionDialog}
          >
            <PlusIcon />
          </button>
        </nav>
      )}
      <main className="main-area" aria-label="Terminals">
        <PersistenceBanner />
        <RestoreBanner />
        <div className="layout-root">
          {workspace && <LayoutView node={workspace.layout.root} workspace={workspace} />}
          {state.status === "failed" && (
            <p className="layout-error" role="alert">
              termx could not load its sessions: {state.message}
            </p>
          )}
        </div>
        <Notices />
      </main>
      {ui.dialog.kind === "newSession" && <NewSessionDialog />}
      {ui.dialog.kind === "confirmClose" && <ConfirmCloseDialog session={ui.dialog.session} />}
      {ui.dialog.kind === "confirmRestart" && <ConfirmRestartDialog session={ui.dialog.session} />}
      {ui.dialog.kind === "confirmQuit" && (
        // A repeated close or quit shows it afresh, with the focus on Quit.
        <ConfirmQuitDialog key={ui.dialog.asked} running={ui.dialog.running} />
      )}
    </div>
  );
}

/** Listening to an event failed: it was logged where it failed. */
function ignoreFailure(): void {
  // Nothing to undo.
}
