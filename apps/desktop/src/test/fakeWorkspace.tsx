import { act, render } from "@testing-library/react";
import { vi } from "vitest";

import { App } from "../App";
import type { Binding } from "../keybindings/keybindings";
import type { Session, SessionId, WorkspaceDelta, WorkspaceSnapshot } from "../lib/ipc";
import type { Platform } from "../lib/platform";
import { type FocusSource, manualFocusSource } from "../lib/windowFocus";
import { type ControllerFactory, SessionStore } from "../terminal/sessionStore";
import type { AutoStart, TerminalController, TerminalStatus } from "../terminal/TerminalController";
import { Workbench } from "../workspace/workbench";
import { type WorkspaceBackend, WorkspaceStore } from "../workspace/workspaceStore";
import { fakeClaude } from "./fakeClaude";
import { recordIPC } from "./ipcRecorder";
import { settle } from "./terminalHarness";

/**
 * A workspace on the Rust side, as far as the interface can tell: it
 * records every command, answers with what a test says, and sends deltas
 * when a test pushes them. It never changes the workspace on its own: a
 * test plays the part of the Rust side by pushing the delta that follows.
 */

export interface BackendCall {
  command: keyof WorkspaceBackend;
  args: unknown[];
}

type Responders = Partial<{
  [K in keyof WorkspaceBackend]: (...args: Parameters<WorkspaceBackend[K]>) => unknown;
}>;

export function fakeWorkspace(initial: WorkspaceSnapshot, responders: Responders = {}) {
  const calls: BackendCall[] = [];
  /** What the Rust side would emit: the menu bar and quitting. */
  const events: { menu?: (command: string) => void; quit?: (running: number) => void } = {};
  let listener: ((delta: WorkspaceDelta) => void) | null = null;

  const record =
    (command: keyof WorkspaceBackend, fallback?: unknown) =>
    (...args: unknown[]): Promise<never> => {
      calls.push({ command, args });
      const responder: ((...a: unknown[]) => unknown) | undefined = responders[command] as
        ((...a: unknown[]) => unknown) | undefined;
      try {
        return Promise.resolve((responder ? responder(...args) : fallback) as never);
      } catch (error) {
        // A test throws what the Rust side rejects with: a `CommandError`.
        // eslint-disable-next-line @typescript-eslint/prefer-promise-reject-errors
        return Promise.reject(error);
      }
    };

  const backend: WorkspaceBackend = {
    subscribeWorkspace: (onDelta) => {
      listener = onDelta;
      calls.push({ command: "subscribeWorkspace", args: [] });
      return Promise.resolve(initial);
    },
    refreshGit: record("refreshGit"),
    pickFolder: record("pickFolder", null),
    forgetRecentFolder: record("forgetRecentFolder", true),
    createSession: record("createSession"),
    createSiblingSession: record("createSiblingSession"),
    renameSession: record("renameSession"),
    reorderSession: record("reorderSession"),
    reorderProject: record("reorderProject"),
    closeSession: record("closeSession"),
    stopSession: record("stopSession"),
    splitPane: record("splitPane", 2),
    closePane: record("closePane"),
    movePane: record("movePane"),
    resizeSplit: record("resizeSplit", 0.5),
    focusPane: record("focusPane"),
    showSession: record("showSession"),
    quitApp: record("quitApp"),
    cancelQuit: record("cancelQuit"),
    quitShown: record("quitShown"),
    setMenuShortcuts: record("setMenuShortcuts"),
    onQuitRequested: (handler) => {
      events.quit = handler;
      return Promise.resolve(() => undefined);
    },
    onMenuCommand: (handler) => {
      events.menu = handler;
      return Promise.resolve(() => undefined);
    },
  };

  /** Sends a delta, as the Rust side does after a change. */
  const push = (delta: Partial<WorkspaceDelta>) => {
    if (!listener) throw new Error("nobody subscribed to the workspace");
    listener({
      revision: 1_000 + calls.length,
      sessions: [],
      removed: [],
      sessionOrder: null,
      projects: null,
      layout: null,
      recentFolders: null,
      persistence: null,
      ...delta,
    });
  };

  /** The calls of one command, by their arguments. */
  const callsOf = (command: keyof WorkspaceBackend) =>
    calls.filter((call) => call.command === command).map((call) => call.args);

  return { backend, calls, push, callsOf, events };
}

/** A stand-in for the terminal of a session. */
export interface FakeController {
  sessionId: SessionId;
  autoStart: AutoStart;
  status: TerminalStatus;
  terminal: { cols: number; rows: number };
  attach: ReturnType<typeof vi.fn>;
  detach: ReturnType<typeof vi.fn>;
  focus: ReturnType<typeof vi.fn>;
  dispose: ReturnType<typeof vi.fn>;
  startDetached: ReturnType<typeof vi.fn>;
  reattach: ReturnType<typeof vi.fn>;
  getStatus: () => TerminalStatus;
}

export function fakeTerminals() {
  const controllers: FakeController[] = [];
  const factory: ControllerFactory = (sessionId, autoStart) => {
    const controller: FakeController = {
      sessionId,
      autoStart,
      status: { kind: "idle" },
      terminal: { cols: 100, rows: 30 },
      attach: vi.fn(),
      detach: vi.fn(),
      focus: vi.fn(),
      dispose: vi.fn(),
      startDetached: vi.fn((mode: string) => {
        controller.status = { kind: "starting" };
        return mode;
      }),
      reattach: vi.fn(() => {
        controller.status = { kind: "running", terminal: 99 };
      }),
      getStatus: () => controller.status,
    };
    controllers.push(controller);
    return controller as unknown as TerminalController;
  };
  const store = new SessionStore(factory);
  const of = (id: SessionId) => controllers.find((controller) => controller.sessionId === id);
  return { store, controllers, of };
}

/** Renders the window on a fake workspace, and waits for it to load. */
export async function renderApp(
  workspace: WorkspaceSnapshot,
  options: {
    responders?: Responders;
    platform?: Platform;
    bindings?: Binding[];
    /** The Claude Code integration; installed, with consent, when omitted. */
    claude?: ReturnType<typeof fakeClaude>;
    /** Whether the user can be looking at the window; yes when omitted. */
    focus?: FocusSource;
  } = {},
) {
  const ipc = recordIPC((command) => (command === "core_version" ? "0.0.0" : undefined));
  const fake = fakeWorkspace(workspace, options.responders);
  const terminals = fakeTerminals();
  const claude = options.claude ?? fakeClaude();
  const workbench = new Workbench(new WorkspaceStore(fake.backend), terminals.store, {
    claude: claude.backend,
    focus: options.focus ?? manualFocusSource(),
  });
  const view = render(
    <App
      workbench={workbench}
      platform={options.platform ?? "linux"}
      bindings={options.bindings}
    />,
  );
  await act(async () => {
    await workbench.bootstrap();
    await settle();
  });
  const push = (delta: Partial<WorkspaceDelta>) => {
    act(() => {
      fake.push(delta);
    });
  };
  /** Replaces sessions, as a delta. */
  const update = (...sessions: Session[]) => {
    push({ sessions });
  };
  return { ...fake, ...terminals, ipc, workbench, view, push, update, claude };
}

/** Lets the calls in flight finish, inside `act`. */
export async function flush(): Promise<void> {
  await act(async () => {
    await settle();
  });
}
