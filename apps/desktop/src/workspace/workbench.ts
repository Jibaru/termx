import { attentionQueue } from "../attention/agent";
import { AttentionMonitor } from "../attention/monitor";
import { NotificationPrefs } from "../attention/notificationPrefs";
import { type ClaudeBackend, NO_CLAUDE } from "../claude/backend";
import { ClaudeIntegrationStore } from "../claude/integration";
import { reportError } from "../lib/errorReporting";
import {
  isCommandError,
  type NewSession,
  type Session,
  type SessionId,
  type SplitDirection,
  type SplitPlacement,
  type WorkspaceSnapshot,
} from "../lib/ipc";
import { type SessionStore, sessionToRestore } from "../terminal/sessionStore";
import { describeCommandError, isLive } from "./describe";
import { beforeFor, buildSidebar } from "./grouping";
import {
  type Direction,
  neighbourPane,
  paneOfSession,
  sessionOfPane,
  visibleSessions,
} from "./layoutTree";
import type { WorkspaceStore } from "./workspaceStore";

/**
 * What the user can do, in one place: every button, menu entry and
 * shortcut calls one of these. Each goes to the workspace on the Rust side,
 * whose answer arrives as a delta; nothing here changes the workspace on
 * its own.
 *
 * A command that fails is shown to the user in the words of its kind
 * (`describeCommandError`). Command errors were logged by the Rust side
 * already; anything else that goes wrong is sent to `reportError`.
 */

/** How many running sessions off the screen are attached to at a time after a reload. */
export const REATTACH_BATCH = 4;
/** How long between two such batches. */
export const REATTACH_INTERVAL_MS = 250;

export interface Notice {
  id: number;
  text: string;
}

export type DialogState =
  | { kind: "none" }
  | { kind: "newSession" }
  | { kind: "confirmClose"; session: SessionId }
  | { kind: "confirmRestart"; session: SessionId }
  | {
      kind: "confirmQuit";
      running: number;
      /** Counts the requests, so that a repeat shows the question afresh. */
      asked: number;
    };

/** Something said once: `id` tells two equal texts apart. */
export interface Message {
  id: number;
  text: string;
}

export interface UiState {
  dialog: DialogState;
  /** The session whose name is being edited in the sidebar. */
  renaming: SessionId | null;
  notices: readonly Notice[];
  /** The session being dragged from the sidebar, for the drop zones of the panes. */
  dragging: SessionId | null;
  /** The sessions restored from the last run, for the restore banner. */
  restored: readonly SessionId[];
  /** The restore banner was dismissed. */
  restoreDismissed: boolean;
  /** The last thing said to screen readers (a live region). */
  announcement: Message | null;
  /** A short hint on screen, such as "No session needs you right now". */
  hint: Message | null;
}

const INITIAL_UI: UiState = {
  dialog: { kind: "none" },
  renaming: null,
  notices: [],
  dragging: null,
  restored: [],
  restoreDismissed: false,
  announcement: null,
  hint: null,
};

export interface WorkbenchOptions {
  /** The commands of the Claude Code integration; none when omitted. */
  claude?: ClaudeBackend;
  /** Whether the window has the focus. */
  windowFocused?: () => boolean;
}

export class Workbench {
  private ui: UiState = INITIAL_UI;
  private readonly listeners = new Set<() => void>();
  private nextNotice = 1;
  private nextMessage = 1;
  private bootstrapped: Promise<void> | null = null;
  /** The Claude Code integration: whether agents report, and its setup. */
  readonly claude: ClaudeIntegrationStore;
  /** When to send OS notifications. */
  readonly notificationPrefs = new NotificationPrefs();
  /** Notifications, the badge and announcements about sessions that wait. */
  readonly attention: AttentionMonitor;

  constructor(
    readonly workspace: WorkspaceStore,
    readonly terminals: SessionStore,
    options: WorkbenchOptions = {},
  ) {
    this.claude = new ClaudeIntegrationStore(options.claude ?? NO_CLAUDE);
    this.attention = new AttentionMonitor({
      workspace,
      claude: this.claude,
      prefs: this.notificationPrefs,
      windowFocused: options.windowFocused ?? (() => document.hasFocus()),
      announce: this.announce,
    });
  }

  private get backend() {
    return this.workspace.backend;
  }

  // --- UI state --------------------------------------------------------------

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getUi = (): UiState => this.ui;

  private setUi(change: Partial<UiState>): void {
    this.ui = { ...this.ui, ...change };
    for (const listener of this.listeners) listener();
  }

  openNewSessionDialog = (): void => {
    this.setUi({ dialog: { kind: "newSession" } });
  };

  closeDialog = (): void => {
    // No to quitting: the next close asks again.
    if (this.ui.dialog.kind === "confirmQuit") {
      void this.run("cancel-quit", () => this.backend.cancelQuit());
    }
    this.setUi({ dialog: { kind: "none" } });
    this.focusTerminal();
  };

  startRenaming = (id: SessionId): void => {
    this.setUi({ renaming: id });
  };

  stopRenaming = (): void => {
    this.setUi({ renaming: null });
  };

  setDragging = (id: SessionId | null): void => {
    if (this.ui.dragging !== id) this.setUi({ dragging: id });
  };

  dismissRestore = (): void => {
    this.setUi({ restoreDismissed: true });
  };

  notify = (text: string): void => {
    const notice = { id: this.nextNotice++, text };
    this.setUi({ notices: [...this.ui.notices, notice].slice(-3) });
  };

  dismissNotice = (id: number): void => {
    this.setUi({ notices: this.ui.notices.filter((notice) => notice.id !== id) });
  };

  /** Says `text` to screen readers, politely. */
  announce = (text: string): void => {
    if (text === "") return;
    this.setUi({ announcement: { id: this.nextMessage++, text } });
  };

  /** Shows a short hint, which is also announced. */
  hint = (text: string): void => {
    const message = { id: this.nextMessage++, text };
    this.setUi({ hint: message, announcement: message });
  };

  dismissHint = (id: number): void => {
    if (this.ui.hint?.id === id) this.setUi({ hint: null });
  };

  /** Runs a command, telling the user if it fails. Never rejects. */
  private async run<T>(source: string, action: () => Promise<T>): Promise<T | undefined> {
    try {
      return await action();
    } catch (error) {
      this.fail(source, error);
      return undefined;
    }
  }

  private fail(source: string, error: unknown): void {
    if (!isCommandError(error)) reportError(source, error);
    this.notify(describeCommandError(error));
  }

  // --- Reading the workspace ---------------------------------------------------

  private get current(): WorkspaceSnapshot | null {
    return this.workspace.workspace;
  }

  session(id: SessionId): Session | undefined {
    return this.current?.sessions.find((session) => session.id === id);
  }

  /** The session of the focused pane, if it shows one. */
  focusedSession(): Session | undefined {
    const workspace = this.current;
    if (!workspace) return undefined;
    const id = sessionOfPane(workspace.layout, workspace.layout.focused);
    return id === null ? undefined : this.session(id);
  }

  private focusTerminal(): void {
    const session = this.focusedSession();
    if (session) this.terminals.get(session.id)?.focus();
  }

  // --- Start-up --------------------------------------------------------------

  /**
   * Subscribes to the workspace and makes sure there is something to look
   * at: a new shell in an empty workspace, or the session termx last had
   * on screen when no pane shows one. Restored sessions are dormant and stay
   * so: nothing is started on its own. Runs once.
   */
  bootstrap(): Promise<void> {
    this.bootstrapped ??= this.bootstrapOnce();
    return this.bootstrapped;
  }

  private async bootstrapOnce(): Promise<void> {
    this.attention.start();
    void this.claude.load();
    const workspace = await this.workspace.start();
    if (!workspace) {
      this.bootstrapped = null;
      return;
    }
    this.setUi({
      restored: workspace.sessions
        .filter((session) => session.state.status === "dormant")
        .map((session) => session.id),
    });
    // Sessions that run already: this page was reloaded. Each is attached
    // to again, so that none is held back or restarted: those on screen at
    // once, the others a few at a time (each repaint is up to 256 KiB and
    // a redraw of its program). In between they show as running, and one
    // shown before its turn is attached by its pane.
    const live = workspace.sessions.filter(
      (session) => isLive(session) && session.terminalId !== null,
    );
    const shown = visibleSessions(workspace.layout);
    for (const session of live) if (shown.has(session.id)) this.terminals.reattach(session);
    this.reattachLater(
      live.filter((session) => !shown.has(session.id)).map((session) => session.id),
    );
    if (workspace.sessions.length === 0) {
      await this.run("bootstrap", () => this.backend.createSession({ kind: "shell" }));
      return;
    }
    const restore = sessionToRestore(workspace);
    if (shown.size === 0 && restore) {
      await this.run("bootstrap", () =>
        this.backend.showSession(workspace.layout.focused, restore.id),
      );
    }
  }

  /** Attaches to running sessions off the screen, `REATTACH_BATCH` at a time. */
  private reattachLater(queue: SessionId[]): void {
    if (queue.length === 0) return;
    setTimeout(() => {
      for (const id of queue.slice(0, REATTACH_BATCH)) {
        const session = this.session(id);
        if (session && isLive(session)) this.terminals.reattach(session);
      }
      this.reattachLater(queue.slice(REATTACH_BATCH));
    }, REATTACH_INTERVAL_MS);
  }

  // --- Sessions --------------------------------------------------------------

  /**
   * Creates a session from the new-session dialog and shows it in the
   * focused pane. Rejects, for the dialog to say why.
   */
  async createSession(request: NewSession): Promise<Session> {
    const session = await this.backend.createSession({ ...request, show: true });
    this.setUi({ dialog: { kind: "none" } });
    return session;
  }

  /**
   * Another Claude Code session in the folder of the focused one, without
   * asking anything. With nothing focused, the new-session dialog opens.
   */
  createSibling = async (from?: SessionId): Promise<void> => {
    const source = from ?? this.focusedSession()?.id;
    if (source === undefined) {
      this.openNewSessionDialog();
      return;
    }
    // The Rust side shows it in the focused pane; the session it replaces
    // there keeps running, a click away in the sidebar.
    await this.run("create-sibling", () => this.backend.createSiblingSession(source));
  };

  /**
   * Puts a session in front of the user: the pane that shows it gets the
   * focus, or the focused pane shows it.
   */
  activate = async (id: SessionId): Promise<void> => {
    const workspace = this.current;
    if (!workspace) return;
    const pane = paneOfSession(workspace.layout, id);
    if (pane === null) {
      await this.run("show-session", () => this.backend.showSession(workspace.layout.focused, id));
    } else if (pane !== workspace.layout.focused) {
      await this.run("focus-pane", () => this.backend.focusPane(pane));
    }
    this.terminals.get(id)?.focus();
  };

  /** Shows a session in the focused pane, moving it if another pane has it. */
  showInFocusedPane = async (id: SessionId): Promise<void> => {
    const workspace = this.current;
    if (!workspace) return;
    await this.run("show-session", () => this.backend.showSession(workspace.layout.focused, id));
  };

  /** Shows a session in `pane`. */
  showInPane = async (pane: number, id: SessionId | null): Promise<void> => {
    await this.run("show-session", () => this.backend.showSession(pane, id));
  };

  rename = async (id: SessionId, name: string): Promise<void> => {
    this.setUi({ renaming: null });
    const trimmed = name.trim();
    // An empty name gives the automatic title back.
    await this.run("rename-session", () =>
      this.backend.renameSession(id, trimmed === "" ? null : trimmed),
    );
  };

  /**
   * Closes a session, after asking when something runs in it: its process
   * would be killed.
   */
  requestClose = async (id?: SessionId): Promise<void> => {
    const session = id === undefined ? this.focusedSession() : this.session(id);
    if (!session) return;
    if (isLive(session)) {
      this.setUi({ dialog: { kind: "confirmClose", session: session.id } });
      return;
    }
    await this.close(session.id);
  };

  close = async (id: SessionId): Promise<void> => {
    if (this.ui.dialog.kind === "confirmClose") this.setUi({ dialog: { kind: "none" } });
    await this.run("close-session", () => this.backend.closeSession(id));
  };

  stop = async (id: SessionId): Promise<void> => {
    await this.run("stop-session", () => this.backend.stopSession(id));
  };

  /** Starts a session that does not run, carrying on where it was. */
  resume = (id: SessionId): void => {
    const session = this.session(id);
    // One that runs is left alone: resuming it would restart it.
    if (session && !isLive(session)) this.terminals.resume(session);
  };

  /**
   * Starts a session again from scratch, after asking when it runs: its
   * process, and whatever Claude Code is in the middle of, would be killed.
   */
  restart = (id: SessionId): void => {
    const session = this.session(id);
    if (!session) return;
    if (isLive(session)) {
      this.setUi({ dialog: { kind: "confirmRestart", session: id } });
      return;
    }
    this.terminals.restart(session);
  };

  /** Restarts a session the user confirmed restarting. */
  confirmRestart = (id: SessionId): void => {
    this.setUi({ dialog: { kind: "none" } });
    const session = this.session(id);
    if (session) this.terminals.restart(session);
  };

  /** The sessions restored from the last run that still wait to be resumed. */
  waitingRestored(): Session[] {
    const restored = new Set(this.ui.restored);
    return (this.current?.sessions ?? []).filter(
      (session) => restored.has(session.id) && session.state.status === "dormant",
    );
  }

  /** Resumes every session restored from the last run that is still dormant. */
  resumeAll = (): void => {
    for (const session of this.waitingRestored()) this.terminals.resume(session);
    this.setUi({ restoreDismissed: true });
  };

  /** The next or previous session of the sidebar, shown in front. */
  cycle = async (step: 1 | -1): Promise<void> => {
    const workspace = this.current;
    if (!workspace) return;
    const { order } = buildSidebar(workspace);
    if (order.length === 0) return;
    const current = sessionOfPane(workspace.layout, workspace.layout.focused);
    const index = current === null ? -1 : order.indexOf(current);
    const next =
      index === -1
        ? step === 1
          ? order[0]
          : order.at(-1)
        : order[(index + step + order.length) % order.length];
    if (next !== undefined) await this.activate(next);
  };

  /** The `n`th session of the sidebar, counting from 1. */
  jumpTo = async (n: number): Promise<void> => {
    const workspace = this.current;
    if (!workspace) return;
    const id = buildSidebar(workspace).order[n - 1];
    if (id !== undefined) await this.activate(id);
  };

  // --- Attention ---------------------------------------------------------------

  /**
   * The sessions that need the user, the one waiting longest first. Empty
   * while the plugin of Claude Code does not report.
   */
  waitingSessions(): Session[] {
    const workspace = this.current;
    if (!workspace || !this.claude.reporting) return [];
    return attentionQueue(workspace);
  }

  /**
   * Shows the session that has waited longest for the user. From a session
   * that waits, goes on to the next one in that order: pressed again, the
   * shortcut goes through all of them.
   */
  jumpToWaiting = async (): Promise<void> => {
    const queue = this.waitingSessions();
    if (queue.length === 0) {
      this.hint("No session needs you right now.");
      return;
    }
    const current = this.focusedSession()?.id;
    const index = queue.findIndex((session) => session.id === current);
    const next = queue[(index + 1) % queue.length];
    if (next) await this.activate(next.id);
  };

  /** Shows the next session that needs the user, in the order of the sidebar. */
  nextWaiting = async (): Promise<void> => {
    const workspace = this.current;
    const waiting = new Set(this.waitingSessions().map((session) => session.id));
    if (!workspace || waiting.size === 0) {
      this.hint("No session needs you right now.");
      return;
    }
    const order = buildSidebar(workspace).order;
    const current = this.focusedSession()?.id;
    const start = current === undefined ? -1 : order.indexOf(current);
    for (let step = 1; step <= order.length; step++) {
      const id = order[(start + step + order.length) % order.length];
      if (id !== undefined && waiting.has(id)) {
        await this.activate(id);
        return;
      }
    }
  };

  /** Moves a session up or down among the sessions of its group. */
  moveSession = async (id: SessionId, step: 1 | -1): Promise<void> => {
    const workspace = this.current;
    if (!workspace) return;
    const model = buildSidebar(workspace);
    const siblings =
      model.background.find((session) => session.id === id) !== undefined
        ? model.background.map((session) => session.id)
        : (model.groups
            .find((group) => group.sessions.some((session) => session.id === id))
            ?.sessions.map((session) => session.id) ?? []);
    const index = siblings.indexOf(id);
    if (index === -1) return;
    await this.placeSession(id, siblings, index + step);
  };

  /** Puts `id` at `index` among `siblings`, the sessions of its group. */
  placeSession = async (id: SessionId, siblings: readonly SessionId[], index: number) => {
    if (index < 0 || index >= siblings.length) return;
    const before = beforeFor(siblings, id, index);
    if (before === undefined) return;
    await this.run("reorder-session", () => this.backend.reorderSession(id, before));
  };

  /** Moves a project group up or down. */
  moveProject = async (key: string, step: 1 | -1): Promise<void> => {
    const workspace = this.current;
    if (!workspace) return;
    const keys = buildSidebar(workspace).groups.map((group) => group.project.key);
    const index = keys.indexOf(key);
    if (index === -1) return;
    await this.placeProject(key, keys, index + step);
  };

  placeProject = async (key: string, keys: readonly string[], index: number) => {
    if (index < 0 || index >= keys.length) return;
    const before = beforeFor(keys, key, index);
    if (before === undefined) return;
    await this.run("reorder-project", () => this.backend.reorderProject(key, before));
  };

  // --- Folders ---------------------------------------------------------------

  /** Shows the folder picker. Rejects, for the dialog to say why. */
  pickFolder(): Promise<string | null> {
    return this.backend.pickFolder();
  }

  forgetFolder = async (folder: string): Promise<void> => {
    await this.run("forget-folder", () => this.backend.forgetRecentFolder(folder));
  };

  // --- Panes -----------------------------------------------------------------

  /** Splits the focused pane; the new, empty pane takes the focus. */
  split = async (direction: SplitDirection): Promise<void> => {
    const workspace = this.current;
    if (!workspace) return;
    await this.run("split-pane", () => this.backend.splitPane(workspace.layout.focused, direction));
  };

  /** Splits `pane` and shows `session` in the new half. */
  splitWith = async (
    pane: number,
    direction: SplitDirection,
    placement: SplitPlacement,
    session: SessionId,
  ): Promise<void> => {
    await this.run("split-pane", () =>
      this.backend.splitPane(pane, direction, { placement, session }),
    );
  };

  /**
   * Closes a pane; its session keeps running. Nothing happens to the only
   * pane: Cmd+W, pressed out of habit, must not empty the window.
   */
  closePane = async (pane?: number): Promise<void> => {
    const workspace = this.current;
    if (!workspace || workspace.layout.root.type === "pane") return;
    const target = pane ?? workspace.layout.focused;
    await this.run("close-pane", () => this.backend.closePane(target));
  };

  focusPane = async (pane: number): Promise<void> => {
    const workspace = this.current;
    if (!workspace || workspace.layout.focused === pane) return;
    await this.run("focus-pane", () => this.backend.focusPane(pane));
  };

  /** Gives the focus to the pane next to the focused one. */
  focusDirection = async (direction: Direction): Promise<void> => {
    const workspace = this.current;
    if (!workspace) return;
    const next = neighbourPane(workspace.layout.root, workspace.layout.focused, direction);
    if (next !== null) await this.focusPane(next);
  };

  resizeSplit = async (split: number, ratio: number): Promise<void> => {
    await this.run("resize-split", () => this.backend.resizeSplit(split, ratio));
  };

  /** The window is closing, or termx quitting, while sessions run: ask. */
  requestQuit = (running: number): void => {
    const asked = this.ui.dialog.kind === "confirmQuit" ? this.ui.dialog.asked + 1 : 1;
    this.setUi({ dialog: { kind: "confirmQuit", running, asked } });
    // From now on a repeat shows the question again instead of quitting.
    void this.run("quit-shown", () => this.backend.quitShown());
  };

  /** The user confirmed quitting. */
  quit = async (): Promise<void> => {
    this.setUi({ dialog: { kind: "none" } });
    await this.run("quit", () => this.backend.quitApp());
  };

  /** Reads the Git branches again, for when the window gets the focus back. */
  refreshGit = (): void => {
    void this.run("refresh-git", () => this.backend.refreshGit());
  };
}
