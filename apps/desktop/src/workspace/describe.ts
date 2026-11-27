import {
  type AgentState,
  type CommandError,
  describeError,
  isCommandError,
  type LaunchFailure,
  type PersistenceMode,
  type Session,
  type SessionKind,
} from "../lib/ipc";

/** Words for the user about sessions, their state and what went wrong. */

export const KIND_LABELS: Record<SessionKind, string> = {
  claude: "Claude Code",
  shell: "Shell",
  background: "Background command",
};

/** How a state is coloured: from quiet to loud. */
export type Tone = "neutral" | "busy" | "ok" | "warn" | "error";

export interface StatusText {
  /** Short, for the row. */
  label: string;
  tone: Tone;
  /** Longer, for the tooltip, when there is more to say. */
  detail?: string;
}

/** The short name of each kind of failure. */
const FAILURE_LABELS: Record<LaunchFailure["kind"], string> = {
  claudeNotFound: "Claude Code is not installed",
  folderMissing: "Folder no longer exists",
  invalidCommand: "The command is not valid",
  spawnFailed: "Could not be started",
  launchUnconfirmed: "Your shell profile did not run the command",
};

/** The short name of a failure; a kind not known here uses its message. */
export function failureLabel(failure: LaunchFailure): string {
  return (FAILURE_LABELS as Partial<Record<string, string>>)[failure.kind] ?? failure.message;
}

/** What the row says about the process of a session. */
export function describeState(session: Session): StatusText {
  const state = session.state;
  switch (state.status) {
    case "dormant":
      return { label: "Not running", tone: "neutral", detail: "Resume it to start it again." };
    case "starting":
      return { label: "Starting", tone: "busy" };
    case "exited": {
      if (state.failure) {
        return { label: failureLabel(state.failure), tone: "error", detail: state.failure.message };
      }
      if (state.signal !== null) {
        return { label: `Terminated (${state.signal})`, tone: "error" };
      }
      if (state.code === null || state.code === 0) return { label: "Exited", tone: "neutral" };
      return { label: `Exited with code ${String(state.code)}`, tone: "error" };
    }
    default:
      // `running`, and any status this version does not know.
      if (session.warning) {
        return {
          label: failureLabel(session.warning),
          tone: "warn",
          detail: session.warning.message,
        };
      }
      return { label: "Running", tone: "ok" };
  }
}

/** Whether a session has a process, or is about to. */
export function isLive(session: Session): boolean {
  return session.state.status !== "dormant" && session.state.status !== "exited";
}

export interface AgentText {
  label: string;
  /** `attention` is loud: the agent cannot go on without the user. */
  tone: "busy" | "attention" | "ok" | "neutral";
  /** A status this version does not know. */
  unknown: boolean;
}

/** What the agent slot says. `null` when there is nothing to say. */
export function describeAgent(agent: AgentState | null): AgentText | null {
  if (agent === null) return null;
  switch (agent.status as string) {
    case "working":
      return { label: "Working", tone: "busy", unknown: false };
    case "waitingForPermission":
      return { label: "Needs permission", tone: "attention", unknown: false };
    case "waitingForInput":
      return { label: "Waiting for input", tone: "attention", unknown: false };
    case "done":
      return { label: "Done", tone: "ok", unknown: false };
    default:
      return { label: "Unknown", tone: "neutral", unknown: true };
  }
}

const ERROR_LABELS: Partial<Record<CommandError["kind"], string>> = {
  ...FAILURE_LABELS,
  storageFull: "The disk is full",
  notFound: "It no longer exists",
  sessionRunning: "The session is still running",
};

/**
 * A failed command, for the user: the words of its kind, and the message of
 * the Rust side when it says more. Anything else that was thrown shows its
 * own description.
 */
export function describeCommandError(error: unknown): string {
  if (!isCommandError(error)) return describeError(error);
  const label = ERROR_LABELS[error.kind];
  if (!label) return error.message;
  if (error.message === "" || error.message === label) return label;
  return `${label}: ${error.message}`;
}

/** "now", "12s", "5m", "3h", "2d": how long ago `then` was. */
export function relativeTime(now: number, then: number): string {
  const seconds = Math.max(0, Math.floor((now - then) / 1000));
  if (seconds < 10) return "now";
  if (seconds < 60) return `${String(seconds)}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${String(minutes)}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${String(hours)}h`;
  return `${String(Math.floor(hours / 24))}d`;
}

/** The same, in words, for screen readers and tooltips. */
export function relativeTimeLong(now: number, then: number): string {
  const short = relativeTime(now, then);
  if (short === "now") return "just now";
  const unit = short.slice(-1);
  const value = Number(short.slice(0, -1));
  const names: Record<string, string> = { s: "second", m: "minute", h: "hour", d: "day" };
  const name = names[unit] ?? "";
  return `${String(value)} ${name}${value === 1 ? "" : "s"} ago`;
}

/** The last folder name of a path, for a list of folders. */
export function folderName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const parts = trimmed.split(/[\\/]/);
  const last = parts.at(-1) ?? "";
  if (last !== "") return last;
  return trimmed === "" ? path : trimmed;
}

/** What the persistence warning says, or `null` while all is well. */
export function persistenceMessage(persistence: PersistenceMode): string | null {
  switch (persistence.mode) {
    case "primary":
      return null;
    case "failing":
      return `Changes are not being saved: ${persistence.reason}`;
    case "secondary":
      return "This is a second termx window: another one owns the saved sessions, so changes made here will not be saved.";
    case "disabled":
      return `Changes are not being saved: ${persistence.reason}`;
    case "memory":
      return "Changes are not being saved: there is nowhere to save them.";
    default:
      return null;
  }
}
