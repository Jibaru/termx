import { Channel, invoke, type InvokeArgs } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";
import { openUrl } from "@tauri-apps/plugin-opener";

import { LogLimiter, type LogLimits } from "./logLimiter";
import { describeIpcFailure } from "./logSafe";

/**
 * Typed wrappers around the Tauri commands exposed by the Rust side
 * (apps/desktop/src-tauri/src/commands). Components call these instead of
 * using `invoke` directly, so command names live in a single place.
 *
 * Wrappers go through `call`, which records failed calls in the log.
 */

/** Returns the version of the `termx-core` library the app was built with. */
export function getCoreVersion(): Promise<string> {
  return call<string>("core_version");
}

/** Severity of a record sent to the log by the frontend. */
export type FrontendLogLevel = "error" | "warn" | "info";

/** A record for the log file. Mirrors `FrontendEvent` on the Rust side. */
export interface FrontendEvent {
  level: FrontendLogLevel;
  /** Where it was caught, such as `window.onerror`, `react` or `ipc`. */
  source: string;
  message: string;
  stack?: string;
  /** React component stack, for errors caught by an error boundary. */
  componentStack?: string;
  /** Name of the Tauri command, for failed IPC calls. */
  command?: string;
}

/**
 * What kind of failure a command had, to act on without reading the
 * message. Mirrors `ErrorKind` in Rust. More may be added: treat a kind you
 * do not know as `internal`.
 *
 * - `claudeNotFound`, `folderMissing`, `invalidCommand`, `spawnFailed`,
 *   `launchUnconfirmed`: as the `LaunchFailure` of the same kind.
 * - `storageFull`: the disk is full.
 * - `notFound`: the session, project, pane or split does not exist.
 * - `invalidArgument`: an argument is not acceptable.
 * - `sessionRunning`: the session runs, and the operation needs it stopped.
 * - `internal`: anything else, including a command that panicked.
 */
export type CommandErrorKind =
  | "claudeNotFound"
  | "folderMissing"
  | "invalidCommand"
  | "spawnFailed"
  | "launchUnconfirmed"
  | "storageFull"
  | "notFound"
  | "invalidArgument"
  | "sessionRunning"
  | "internal";

export const COMMAND_ERROR_KINDS = [
  "claudeNotFound",
  "folderMissing",
  "invalidCommand",
  "spawnFailed",
  "launchUnconfirmed",
  "storageFull",
  "notFound",
  "invalidArgument",
  "sessionRunning",
  "internal",
] as const satisfies readonly CommandErrorKind[];

/** What a failed command rejects with. Mirrors `CommandError` in Rust. */
export interface CommandError {
  /** The name of the command. */
  command: string;
  kind: CommandErrorKind;
  /** What happened, for the user. */
  message: string;
}

/** Where a diagnostics bundle was written. */
export interface ExportedBundle {
  /** Full path of the zip file. */
  path: string;
}

const LOG_COMMAND = "log_frontend_event";

let logLimiter = new LogLimiter();

/**
 * Writes a record to the log file. This is the only way the frontend logs.
 *
 * It never throws and never rejects, because a logging problem must not
 * cause another error. Records over the limit are dropped here, before they
 * reach the Rust side, which applies its own rate limit as well. Drops are
 * counted, and the count is logged before the next record that gets through.
 *
 * Privacy: never pass terminal output, typed input, environment variable
 * values or file contents. See docs/logging.md.
 */
export async function logFrontendEvent(event: FrontendEvent): Promise<void> {
  const dropped = logLimiter.admit(event.level, `${event.source}\n${event.message}`);
  if (dropped === null) return;

  try {
    const total = dropped.error + dropped.warn + dropped.info;
    if (total > 0) {
      await invoke(LOG_COMMAND, {
        event: {
          level: "warn",
          source: "frontend-limit",
          message:
            `${String(total)} records were dropped by the frontend rate limit ` +
            `(${String(dropped.error)} errors, ${String(dropped.warn)} warnings, ` +
            `${String(dropped.info)} others)`,
        } satisfies FrontendEvent,
      });
    }
    await invoke(LOG_COMMAND, { event });
  } catch {
    // Nowhere left to report to.
  }
}

/** Starts the rate limit over, optionally with other limits. For tests. */
export function resetFrontendLogLimit(limits?: LogLimits, now?: () => number): void {
  logLimiter = new LogLimiter(limits, now);
}

/** Opens the folder that holds the log files in the file manager. */
export function openLogFolder(): Promise<void> {
  return call("open_log_folder");
}

/**
 * Writes a diagnostics bundle (a zip with the log files and
 * `system-info.json`) to the Downloads folder, opens that folder and returns
 * where the bundle is. The Rust side chooses the location; it cannot be set
 * from here.
 */
export function exportDiagnostics(): Promise<ExportedBundle> {
  return call<ExportedBundle>("export_diagnostics");
}

// --- The workspace ---------------------------------------------------------
//
// The types below mirror what the Rust side sends
// (`termx_core::workspace`). `ipc.contract.json` holds one of each, written
// by a Rust test, and `ipc.contract.test.ts` checks the types against it:
// change both sides together.

/**
 * Identifies a session for its whole life, including across restarts of
 * its process and of termx. Not to be confused with the number of its
 * terminal, which changes every time the session is started.
 */
export type SessionId = string;

/** What a session runs. */
export type SessionKind = "claude" | "shell" | "background";

export const SESSION_KINDS = [
  "claude",
  "shell",
  "background",
] as const satisfies readonly SessionKind[];

/**
 * How a session is started: `fresh` from scratch (which is also how it is
 * restarted), `resume` to carry on. Resuming makes Claude Code continue its
 * conversation; a shell or a background command starts anew in the same
 * folder either way.
 */
export type LaunchMode = "fresh" | "resume";

/**
 * Why a session could not do its job. `message` is written for the user
 * and can be shown as it is.
 */
export interface LaunchFailure {
  /**
   * `launchUnconfirmed`: the user's shell did not get to the command, or
   * not in time, typically because a file it reads at start-up starts
   * another shell, stops with an error or waits for an answer. While the
   * session runs it is a `warning`, cleared if the command starts after
   * all; at the exit it is the `failure`.
   */
  kind: "claudeNotFound" | "folderMissing" | "invalidCommand" | "spawnFailed" | "launchUnconfirmed";
  message: string;
}

export const LAUNCH_FAILURE_KINDS = [
  "claudeNotFound",
  "folderMissing",
  "invalidCommand",
  "spawnFailed",
  "launchUnconfirmed",
] as const satisfies readonly LaunchFailure["kind"][];

/**
 * Where the process of a session is in its life.
 *
 * - `dormant`: nothing runs: restored from the previous run, or stopped.
 *   Offer to resume it. A restored session is never started on its own.
 * - `starting`: created, or asked to start; no process yet.
 * - `running`: the process runs.
 * - `exited`: the process ended, or could not be started (`failure`).
 *
 * It is only about the process: what the agent does is `Session.agent`.
 * Treat a status you do not know as `running`.
 */
export type SessionState =
  | { status: "dormant" }
  | { status: "starting" }
  | { status: "running" }
  | {
      status: "exited";
      /** The exit code; `null` after a signal or when nothing ran. */
      code: number | null;
      /** Description of the signal that ended the process, if any. */
      signal: string | null;
      /** Set when termx knows why the session failed. */
      failure: LaunchFailure | null;
    };

export const SESSION_STATUSES = [
  "dormant",
  "starting",
  "running",
  "exited",
] as const satisfies readonly SessionState["status"][];

/**
 * What the agent of a session is doing, as the hooks of Claude Code will
 * report it. Reserved: `Session.agent` is always `null` until termx
 * installs those hooks. That version only fills it in, adding fields and
 * statuses; `SessionState` stays about the process. Show nothing about
 * the agent while it is `null`. Show a status you do not know as unknown:
 * neutral, neither busy nor asking for attention.
 */
export interface AgentState {
  status: AgentStatus;
  /** Since when it waits for the user, while its status is `waitingFor...`. */
  waitingSince: number | null;
}

/**
 * What the agent does:
 *
 * - `working`: on what it was asked.
 * - `waitingForPermission`: for the user to allow something (a command, an
 *   edit); it cannot go on without them.
 * - `waitingForInput`: for the user's next message or answer.
 * - `done`: it finished and has nothing to do.
 */
export type AgentStatus = "working" | "waitingForPermission" | "waitingForInput" | "done";

export const AGENT_STATUSES = [
  "working",
  "waitingForPermission",
  "waitingForInput",
  "done",
] as const satisfies readonly AgentStatus[];

/** What groups sessions in the sidebar. */
export interface Project {
  /** Sessions with the same key belong together. */
  key: string;
  /** The name to show: the name of the root folder. */
  name: string;
  /** The root of the Git repository, or the folder itself. */
  root: string;
  kind: "git" | "folder";
}

/** What Git says about the folder of a session. */
export interface GitInfo {
  /** The branch checked out; `null` when HEAD is detached or unreadable. */
  branch: string | null;
  /** The abbreviated commit, when HEAD is detached. */
  commit: string | null;
  /** The folder name of the linked worktree; `null` in the main one. */
  worktree: string | null;
}

/** One session: a row of the sidebar. Times are milliseconds since 1970. */
export interface Session {
  id: SessionId;
  kind: SessionKind;
  /** The folder it was created in, where it is started and restarted. */
  cwd: string;
  /** The folder the program last reported (OSC 7) while running, if any. */
  liveCwd: string | null;
  /** The command line of a background session; `null` otherwise. */
  command: string | null;
  /** What to show: `name`, else `autoTitle`, else a default for the kind. */
  title: string;
  /** The name the user gave. It always wins over the automatic title. */
  name: string | null;
  /** The title the program set for its terminal. */
  autoTitle: string | null;
  /** The project of `liveCwd` when known, else of `cwd`. */
  project: Project;
  /** `null` outside a Git repository. */
  git: GitInfo | null;
  state: SessionState;
  /** The number of its terminal while a process runs; `null` otherwise. */
  terminalId: number | null;
  createdAt: number;
  /** When it last printed something, or was last started. */
  lastActivityAt: number;
  /**
   * What the agent of a Claude Code session is doing. Always `null` for
   * now: see `AgentState`.
   */
  agent: AgentState | null;
  /** The conversation Claude Code resumes, once known. */
  claudeSessionId: string | null;
  /**
   * Something to tell the user while the session runs, with a message for
   * them: for now only `launchUnconfirmed`. `null` otherwise.
   */
  warning: LaunchFailure | null;
}

/** How a split arranges its children: side by side, or stacked. */
export type SplitDirection = "row" | "column";

/** A node of the layout tree: a pane, or a split of two nodes. */
export type LayoutNode =
  | {
      type: "pane";
      id: number;
      /** The session shown, if any. */
      session: SessionId | null;
    }
  | {
      type: "split";
      id: number;
      direction: SplitDirection;
      /** The share of `first`, between 0.1 and 0.9. */
      ratio: number;
      /** The left or top child. */
      first: LayoutNode;
      /** The right or bottom child. */
      second: LayoutNode;
    };

/** The layout of the main area. There is always at least one pane. */
export interface Layout {
  root: LayoutNode;
  /** The pane that has the focus. It always exists. */
  focused: number;
  /** Internal: the next identifier the Rust side hands out. */
  nextId: number;
}

/**
 * Whether the workspace is saved between runs.
 *
 * - `primary`: yes.
 * - `failing`: it should be, but saving fails (a full disk, for example):
 *   the changes made since `since` are not saved. termx keeps trying and
 *   goes back to `primary` once a save works. Warn the user while it lasts.
 * - `secondary`: no, because another termx is running and owns the state.
 * - `disabled`: no, because the state file could not be used (`reason`).
 * - `memory`: no, because there is nowhere to save it.
 */
export type PersistenceMode =
  | { mode: "primary" }
  | {
      mode: "failing";
      /** The error of the last save, for the user. */
      reason: string;
      /** When saving started to fail, in milliseconds since 1970. */
      since: number;
    }
  | { mode: "secondary" }
  | { mode: "disabled"; reason: string }
  | { mode: "memory" };

/** Everything needed to draw the workspace. */
export interface WorkspaceSnapshot {
  /** Counts the changes of the workspace. */
  revision: number;
  /** The sessions, in the order the user gave them. */
  sessions: Session[];
  /** The projects of the sessions, in the order the user gave them. */
  projects: Project[];
  layout: Layout;
  /** Folders sessions were created in, most recent first. */
  recentFolders: string[];
  persistence: PersistenceMode;
}

/**
 * What changed in the workspace, with the current values. Parts that did
 * not change are `null`. Apply it with `applyWorkspaceDelta`.
 */
export interface WorkspaceDelta {
  revision: number;
  /** Sessions that were added or changed, as they are now. */
  sessions: Session[];
  /** Sessions that were removed. */
  removed: SessionId[];
  /** All the session ids in order, when sessions were added, removed or reordered. */
  sessionOrder: SessionId[] | null;
  projects: Project[] | null;
  layout: Layout | null;
  recentFolders: string[] | null;
  /** Whether the workspace is saved, when that changed. */
  persistence: PersistenceMode | null;
}

/**
 * The workspace after `delta`. Returns a new snapshot and leaves the one
 * given untouched; sessions that did not change keep their identity, so a
 * component that depends on one of them does not render again.
 *
 * Deltas can be applied as they arrive, in order, without looking at their
 * revisions: each carries current values, and applying one twice changes
 * nothing.
 */
export function applyWorkspaceDelta(
  snapshot: WorkspaceSnapshot,
  delta: WorkspaceDelta,
): WorkspaceSnapshot {
  const byId = new Map(snapshot.sessions.map((session) => [session.id, session]));
  for (const session of delta.sessions) byId.set(session.id, session);
  for (const id of delta.removed) byId.delete(id);

  // The order sent wins. Without one, sessions keep their place and new
  // ones go last, which is where the Rust side puts them.
  const order = delta.sessionOrder ?? [
    ...snapshot.sessions.map((session) => session.id),
    ...delta.sessions.map((session) => session.id),
  ];
  const sessions: Session[] = [];
  for (const id of order) {
    const session = byId.get(id);
    if (session) {
      sessions.push(session);
      byId.delete(id);
    }
  }

  return {
    revision: Math.max(snapshot.revision, delta.revision),
    sessions,
    projects: delta.projects ?? snapshot.projects,
    layout: delta.layout ?? snapshot.layout,
    recentFolders: delta.recentFolders ?? snapshot.recentFolders,
    persistence: delta.persistence ?? snapshot.persistence,
  };
}

/**
 * Subscribes this window to the workspace: resolves with its current
 * state, and calls `onDelta` for every change from then on. Changes that
 * come close together arrive as one delta. There is nothing to poll.
 *
 * `onDelta` can be called before the promise resolves; such a delta is
 * already part of the snapshot, and applying it again is harmless. Keep
 * the deltas that arrive early and apply them once the snapshot is there.
 *
 * A window has one subscription: subscribing again ends the previous one.
 */
export function subscribeWorkspace(
  onDelta: (delta: WorkspaceDelta) => void,
): Promise<WorkspaceSnapshot> {
  const onEvent = new Channel<WorkspaceDelta>();
  onEvent.onmessage = onDelta;
  return call<WorkspaceSnapshot>("workspace_subscribe", { onEvent });
}

/** The workspace as it is now, without subscribing. */
export function getWorkspaceSnapshot(): Promise<WorkspaceSnapshot> {
  return call<WorkspaceSnapshot>("workspace_snapshot");
}

/**
 * Asks for the Git branch of every session to be read again now, for
 * instance when the window gets the focus back. What changed arrives as a
 * delta. termx also does this on its own, every few seconds.
 */
export function refreshGit(): Promise<void> {
  return call("workspace_refresh_git");
}

/**
 * Shows the folder picker of the system. Resolves with the folder chosen,
 * or `null` when the dialog was dismissed.
 */
export function pickFolder(): Promise<string | null> {
  return call<string | null>("folder_pick");
}

/** Removes a folder from the recent folders. Resolves with whether it was one. */
export function forgetRecentFolder(folder: string): Promise<boolean> {
  return call<boolean>("recent_folder_forget", { folder });
}

/** What a new session is. Mirrors `CreateSessionRequest` in Rust. */
export interface NewSession {
  kind: SessionKind;
  /** The folder it runs in. The home directory when omitted. */
  cwd?: string;
  /** The command line; required for `background`, ignored otherwise. */
  command?: string;
  /** A name, as if the user had renamed the session. */
  name?: string;
  /** Whether to show it in the focused pane. Yes when omitted. */
  show?: boolean;
}

/**
 * Creates a session and resolves with it, in the `starting` state. Nothing
 * runs yet: start it with `startSession` once its pane knows its size. Its
 * folder becomes the most recent of the recent folders.
 */
export function createSession(session: NewSession): Promise<Session> {
  return call<Session>("session_create", { request: session });
}

/**
 * Creates another Claude Code session in the folder of the session `id`,
 * without asking anything: "one more in the same project".
 */
export function createSiblingSession(id: SessionId): Promise<Session> {
  return call<Session>("session_create_sibling", { id });
}

/**
 * Gives a session a name, which wins over any automatic title from then
 * on. `null` or an empty name removes it: the automatic title shows again.
 */
export function renameSession(id: SessionId, name: string | null): Promise<void> {
  return call("session_rename", { id, name });
}

/**
 * Moves a session just before the session `before` in the order of the
 * sessions, or to the end when `before` is `null`.
 */
export function reorderSession(id: SessionId, before: SessionId | null): Promise<void> {
  return call("session_reorder", { id, before });
}

/**
 * Moves a project just before the project `before` in the order of the
 * projects, or to the end when `before` is `null`.
 */
export function reorderProject(key: string, before: string | null): Promise<void> {
  return call("project_reorder", { key, before });
}

/**
 * Closes a session for good: its process is killed, and it leaves the
 * sidebar and its pane. Closing a session that is gone does nothing.
 */
export function closeSession(id: SessionId): Promise<void> {
  return call("session_close", { id });
}

/**
 * Stops a session without removing it: its process is killed and it
 * becomes `dormant`, keeping its place, name and conversation. Start it
 * again with `startSession` (`resume` to carry on). Stopping a session that
 * is not running does nothing.
 */
export function stopSession(id: SessionId): Promise<void> {
  return call("session_stop", { id });
}

/**
 * Removes a session that is not running from the sidebar and its pane.
 * Rejects for a running one (kind `sessionRunning`): stop it first, or use
 * `closeSession`.
 */
export function removeSession(id: SessionId): Promise<void> {
  return call("session_remove", { id });
}

/**
 * Tells the Rust side the title a program set for its terminal, as
 * xterm.js reports it (`onTitleChange`). It becomes the title of the
 * session unless the user named it.
 */
export function reportSessionTitle(id: SessionId, title: string): Promise<void> {
  return call("session_report_title", { id, title });
}

/**
 * Tells the Rust side the folder a program says it is in: the payload of
 * an OSC 7 sequence (a `file://` URL) or a path. The project and the
 * branch of the session follow it.
 */
export function reportSessionCwd(id: SessionId, cwd: string): Promise<void> {
  return call("session_report_cwd", { id, cwd });
}

/** How to start a session. Mirrors `StartSessionRequest` in Rust. */
export interface StartSessionOptions {
  /** From scratch when omitted. */
  mode?: LaunchMode;
  /** Width of the terminal, in columns. */
  cols: number;
  /** Height of the terminal, in rows. */
  rows: number;
}

/** The terminal a session was started in. Mirrors `StartedSession` in Rust. */
export interface StartedSession {
  /** The number of the terminal, for `writeSession`, `resizeSession` and `ackSession`. */
  terminal: number;
  /**
   * How many processed bytes to accumulate before acknowledging them with
   * `ackSession`.
   */
  ackBatchBytes: number;
  /** The build number of Windows, or `null` on other systems. */
  windowsBuildNumber: number | null;
}

/** How the process of a session ended. */
export interface SessionExit {
  /** The exit code, or `null` if a signal ended the process. */
  code: number | null;
  /** Description of the signal that ended the process, if any. */
  signal: string | null;
  /** Set when termx knows why the session failed, with a message to show. */
  failure?: LaunchFailure | null;
}

/** Receives what a session produces. */
export interface SessionHandlers {
  /**
   * A batch of output, exactly as the process wrote it. Pass it to the
   * terminal emulator and acknowledge its length with `ackSession` once it
   * was processed. Never log it.
   */
  onOutput: (bytes: Uint8Array) => void;
  /** The process has exited. Nothing follows. */
  onExit: (exit: SessionExit) => void;
}

/** A JSON message of the session channel. Mirrors `SessionEvent` in Rust. */
type SessionEvent = { event: "exit" } & SessionExit;

/**
 * What arrives on the channel of a session: output as raw bytes, events as
 * JSON. The bytes are an `ArrayBuffer`, except through the fallback
 * transport Tauri uses when the webview cannot fetch from the custom
 * protocol, which delivers an array of numbers.
 */
type SessionMessage = ArrayBuffer | number[] | SessionEvent;

function isSessionEvent(message: SessionMessage): message is SessionEvent {
  return !Array.isArray(message) && "event" in message;
}

/**
 * Starts the session `id` in a new terminal and resolves with that
 * terminal. A session that is running is restarted.
 *
 * Output can reach `handlers.onOutput`, and even the exit `handlers.onExit`,
 * before the promise resolves: a process may print, or end, at once. The
 * handlers are called in the order things happened.
 *
 * The state of the session follows on its own (`starting`, `running`,
 * `exited`) and arrives through `subscribeWorkspace`. If the session cannot
 * be started the promise rejects, and its state says why (`failure`).
 */
export function startSession(
  id: SessionId,
  options: StartSessionOptions,
  handlers: SessionHandlers,
): Promise<StartedSession> {
  return call<StartedSession>("session_start", {
    id,
    request: options,
    onEvent: sessionChannel(handlers),
  });
}

/**
 * Attaches to the terminal a session already runs in, as a page does after
 * it was reloaded: nothing is started or restarted. `handlers.onOutput`
 * first receives the recent output of the terminal (up to 256 KiB, with the
 * modes its program set), then what follows. The terminal is resized to
 * `size`, which makes a full-screen program redraw.
 *
 * Rejects with `notFound` when the session does not run.
 */
export function attachSession(
  id: SessionId,
  size: { cols: number; rows: number },
  handlers: SessionHandlers,
): Promise<StartedSession> {
  return call<StartedSession>("session_attach", {
    id,
    cols: size.cols,
    rows: size.rows,
    onEvent: sessionChannel(handlers),
  });
}

/** The channel a session streams to, decoded for `handlers`. */
function sessionChannel(handlers: SessionHandlers): Channel<SessionMessage> {
  const onEvent = new Channel<SessionMessage>();
  onEvent.onmessage = (message) => {
    if (isSessionEvent(message)) {
      handlers.onExit({
        code: message.code,
        signal: message.signal,
        failure: message.failure ?? null,
      });
    } else {
      handlers.onOutput(
        Array.isArray(message) ? Uint8Array.from(message) : new Uint8Array(message),
      );
    }
  };
  return onEvent;
}

/**
 * Sends input to the terminal of a session, as if it had been typed.
 *
 * `data` is text. With `binary`, each of its characters stands for one byte
 * instead, which is how xterm.js reports input that is not text (`onBinary`).
 *
 * Wait for the promise before sending the next input of the same terminal:
 * that is what keeps input in order.
 */
export function writeSession(terminal: number, data: string, binary = false): Promise<void> {
  return call("session_write", { id: terminal, data, binary });
}

/** Changes the size of the terminal of a session. */
export function resizeSession(terminal: number, cols: number, rows: number): Promise<void> {
  return call("session_resize", { id: terminal, cols, rows });
}

/**
 * Tells the terminal of a session that `bytes` of its output were
 * processed. A terminal whose output is not acknowledged is held back, so
 * that a program printing faster than the terminal can draw does not fill
 * the memory.
 */
export function ackSession(terminal: number, bytes: number): Promise<void> {
  return call("session_ack", { id: terminal, bytes });
}

// --- The layout ------------------------------------------------------------
//
// Every operation is validated by the Rust side: one that would break the
// layout rejects and changes nothing. The new layout arrives as a delta.

/** Where a new pane goes: before (left or above) or after (right or below). */
export type SplitPlacement = "before" | "after";

/** Where a moved pane lands on its target. `center` swaps what the two show. */
export type PaneEdge = "left" | "right" | "top" | "bottom" | "center";

/**
 * Splits a pane in two, side by side (`row`) or stacked (`column`), and
 * resolves with the id of the new pane, which takes the focus. The new pane
 * shows `session`, or nothing; it goes after the pane unless `placement`
 * says otherwise.
 */
export function splitPane(
  pane: number,
  direction: SplitDirection,
  options: { placement?: SplitPlacement; session?: SessionId | null } = {},
): Promise<number> {
  return call<number>("layout_split", {
    pane,
    direction,
    placement: options.placement ?? null,
    session: options.session ?? null,
  });
}

/**
 * Closes a pane: its neighbour takes its place. The last pane is emptied
 * instead. The session it showed keeps running.
 */
export function closePane(pane: number): Promise<void> {
  return call("layout_close_pane", { pane });
}

/** Moves a pane to an edge of another, or swaps what the two show. */
export function movePane(pane: number, target: number, edge: PaneEdge): Promise<void> {
  return call("layout_move_pane", { pane, target, edge });
}

/**
 * Sets the share of a split given to its first child. Resolves with the
 * ratio that was set: it is kept between 0.1 and 0.9.
 */
export function resizeSplit(split: number, ratio: number): Promise<number> {
  return call<number>("layout_resize_split", { split, ratio });
}

/** Gives the focus to a pane. */
export function focusPane(pane: number): Promise<void> {
  return call("layout_focus_pane", { pane });
}

/**
 * Shows a session in a pane, or empties the pane (`null`). A session that
 * was shown in another pane moves there.
 */
export function showSession(pane: number, session: SessionId | null): Promise<void> {
  return call("layout_show_session", { pane, session });
}

/**
 * Quits termx, which ends every session; they come back, dormant, the next
 * time it starts. The answer to `onQuitRequested`.
 */
export function quitApp(): Promise<void> {
  return call("app_quit");
}

/**
 * The window shows the question about quitting: a repeated close or quit
 * shows it again instead of going through.
 */
export function quitShown(): Promise<void> {
  return call("app_quit_shown");
}

/**
 * Gives the items of the menu bar (macOS) their shortcuts, from the key
 * bindings of the window. Does nothing without a menu bar.
 */
export function setMenuShortcuts(
  shortcuts: { command: string; accelerator: string | null }[],
): Promise<void> {
  return call("menu_set_shortcuts", { shortcuts });
}

/** The user said no to quitting: the next close or quit asks again. */
export function cancelQuit(): Promise<void> {
  return call("app_quit_cancel");
}

/**
 * Calls `handler` with the number of running sessions when the window is
 * closed, or termx is quit, while sessions run: the user is to confirm it
 * (`quitApp`) or not (nothing to do). Resolves with how to stop listening.
 */
export function onQuitRequested(handler: (running: number) => void): Promise<() => void> {
  return listen<number>("quit-requested", (event) => {
    handler(event.payload);
  });
}

/**
 * Calls `handler` with the id of the menu item chosen in the menu bar of
 * macOS, which is the id of a command of the key bindings. Resolves with
 * how to stop listening.
 */
export function onMenuCommand(handler: (command: string) => void): Promise<() => void> {
  return listen<string>("menu-command", (event) => {
    handler(event.payload);
  });
}

/**
 * Opens a link in the system browser. Only `http` and `https` links are
 * opened: the Rust side refuses everything else.
 */
export function openExternalUrl(url: string): Promise<void> {
  return openUrl(url);
}

/** The text on the system clipboard. */
export function readClipboardText(): Promise<string> {
  return readText();
}

/** Puts `text` on the system clipboard. */
export function writeClipboardText(text: string): Promise<void> {
  return writeText(text);
}

/** Whether a rejected call carries the `CommandError` of a Rust command. */
export function isCommandError(error: unknown): error is CommandError {
  return (
    typeof error === "object" &&
    error !== null &&
    typeof (error as Partial<CommandError>).command === "string" &&
    typeof (error as Partial<CommandError>).kind === "string" &&
    typeof (error as Partial<CommandError>).message === "string"
  );
}

/**
 * A readable description of whatever a call rejected or a `throw` threw, to
 * show in the interface. It may contain the thrown value itself, so it must
 * not be logged: use `describeForLog` from `logSafe.ts` for that.
 */
export function describeError(error: unknown): string {
  if (isCommandError(error)) return error.message;
  if (error instanceof Error) return error.message || error.name;
  if (typeof error === "string") return error;
  // `JSON.stringify` gives no string for undefined, functions and symbols,
  // and throws on cycles.
  if (typeof error !== "object" || error === null) return String(error);
  try {
    return JSON.stringify(error);
  } catch {
    return "error object that cannot be serialised";
  }
}

/**
 * Invokes a Tauri command and records the failure if it rejects.
 *
 * A `CommandError` was already logged by the Rust side, so only failures of
 * the call itself (unknown command, bad arguments, no permission) are logged
 * here. The rejection is passed on to the caller either way.
 */
async function call<T = void>(command: string, args?: InvokeArgs): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    if (!isCommandError(error)) {
      void logFrontendEvent({
        level: "error",
        source: "ipc",
        command,
        ...describeIpcFailure(error),
      });
    }
    throw error;
  }
}
