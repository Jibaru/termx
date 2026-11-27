import type { AgentIconStatus } from "../icons/statusText";
import type { AgentState, Session, WorkspaceSnapshot } from "../lib/ipc";
import { isLive } from "../workspace/describe";
import { buildSidebar } from "../workspace/grouping";

/**
 * What the agent of a session does, as the window shows it, and which
 * sessions need the user.
 *
 * A session needs the user when its agent waits for them (permission or
 * input), its report is fresh (not `stale`), and its process runs. Nothing
 * else is ever loud: a stale report is shown quietly, an unknown status
 * neutrally.
 */

/** A time of the workspace (milliseconds since 1970), or `null` if it is not one. */
export function toMillis(value: number | null | undefined): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

export interface AgentView {
  icon: AgentIconStatus;
  /** Short, for the row: "Needs permission". */
  label: string;
  /** Longer, for tooltips and screen readers: "Waiting for permission". */
  longLabel: string;
  /** How it is coloured. `attention` only while it waits and is fresh. */
  tone: "busy" | "attention" | "ok" | "neutral";
  /** It waits for the user, and the report is fresh. */
  waiting: boolean;
  stale: boolean;
  /** Since when it waits, in milliseconds since 1970. */
  waitingSince: number | null;
  /** Since when it has its status, in milliseconds since 1970. */
  since: number | null;
}

const KNOWN: Record<
  Exclude<AgentIconStatus, "unknown">,
  { label: string; longLabel: string; tone: AgentView["tone"] }
> = {
  working: { label: "Working", longLabel: "Working", tone: "busy" },
  waitingForPermission: {
    label: "Needs permission",
    longLabel: "Waiting for permission",
    tone: "attention",
  },
  waitingForInput: {
    label: "Waiting for input",
    longLabel: "Waiting for input",
    tone: "attention",
  },
  done: { label: "Done", longLabel: "Done", tone: "ok" },
};

/** What to show for `agent`, or `null` when there is nothing to show. */
export function viewAgent(agent: AgentState | null | undefined): AgentView | null {
  if (!agent) return null;
  const status = agent.status as string;
  const known = status in KNOWN ? KNOWN[status as keyof typeof KNOWN] : null;
  const stale = agent.stale;
  const waitingSince = toMillis(agent.waitingSince);
  const since = toMillis(agent.since) ?? waitingSince;
  if (!known) {
    return {
      icon: "unknown",
      label: "Unknown",
      longLabel: "Unknown state",
      tone: "neutral",
      waiting: false,
      stale,
      waitingSince: null,
      since,
    };
  }
  const waiting = known.tone === "attention" && !stale;
  return {
    icon: status as AgentIconStatus,
    label: known.label,
    longLabel: known.longLabel,
    // A stale report is quiet, whatever it says.
    tone: stale ? "neutral" : known.tone,
    waiting,
    stale,
    waitingSince,
    since,
  };
}

/** "under 1 min", "3 min", "2 h", "4 d": how long `ms` is, for tooltips. */
export function formatDuration(ms: number): string {
  const minutes = Math.floor(Math.max(0, ms) / 60_000);
  if (minutes < 1) return "under 1 min";
  if (minutes < 60) return `${String(minutes)} min`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${String(hours)} h`;
  return `${String(Math.floor(hours / 24))} d`;
}

/**
 * The tooltip of an agent state: "Waiting for permission · 3 min", or for
 * a stale one "Waiting for permission · last seen 5 min ago".
 */
export function agentTooltip(view: AgentView, now: number): string {
  if (view.stale) {
    return view.since === null
      ? `${view.longLabel} · no longer reporting`
      : `${view.longLabel} · last seen ${formatDuration(now - view.since)} ago`;
  }
  const from = view.waiting ? (view.waitingSince ?? view.since) : view.since;
  return from === null ? view.longLabel : `${view.longLabel} · ${formatDuration(now - from)}`;
}

/** Whether `session` needs the user now. */
export function isWaiting(session: Session): boolean {
  return isLive(session) && viewAgent(session.agent)?.waiting === true;
}

/** When `session` started to wait, for ordering: the oldest first. */
function waitingKey(session: Session): number {
  const view = viewAgent(session.agent);
  return view?.waitingSince ?? view?.since ?? session.lastActivityAt;
}

/**
 * The sessions that need the user, the one that has waited longest first.
 * Sessions that started to wait at the same time keep the order of the
 * sidebar.
 */
export function attentionQueue(workspace: WorkspaceSnapshot): Session[] {
  const order = buildSidebar(workspace).order;
  const rank = new Map(order.map((id, index) => [id, index]));
  return workspace.sessions
    .filter(isWaiting)
    .sort(
      (a, b) =>
        waitingKey(a) - waitingKey(b) ||
        (rank.get(a.id) ?? Infinity) - (rank.get(b.id) ?? Infinity),
    );
}

/**
 * Identifies one wait of a session, so that a notification is sent once
 * per wait: a new wait has a new `waitingSince`.
 */
export function waitKey(session: Session): string {
  const view = viewAgent(session.agent);
  return `${session.id}\u0000${String(view?.waitingSince ?? view?.icon ?? "")}`;
}
