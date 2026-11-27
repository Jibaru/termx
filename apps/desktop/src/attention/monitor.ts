import type { ClaudeIntegrationStore } from "../claude/integration";
import type { Session, SessionId, WorkspaceSnapshot } from "../lib/ipc";
import { visibleSessions } from "../workspace/layoutTree";
import type { WorkspaceStore } from "../workspace/workspaceStore";
import { attentionQueue, isWaiting, viewAgent, waitKey } from "./agent";
import type { NotificationPrefs } from "./notificationPrefs";

/**
 * Tells the user when a session starts to need them, without crying wolf.
 *
 * - **Once per wait.** A session that starts to wait is signalled once; it
 *   is signalled again only for a new wait (a new `waitingSince`).
 * - **Flapping is ignored.** A wait is signalled after `SETTLE_MS`, and only
 *   if it still lasts: a permission granted at once by a rule never shows.
 *   A session signalled less than `COOLDOWN_MS` ago is not signalled again.
 * - **Bursts are one notification.** The waits that start within
 *   `SETTLE_MS` of the first go out together: "3 sessions need you".
 * - **Not while the user looks at it.** No OS notification for a session
 *   shown in a pane of the focused window. The others, and every session
 *   while the window is in the background, are notified, if the user did
 *   not turn notifications off.
 * - "Done" is signalled the same way when the user turned it on.
 *
 * Every signal is also announced to screen readers (`announce`), and the
 * badge of the app always holds the number of sessions that wait.
 *
 * Nothing is sent before the plugin reports (`reporting`), and the waits
 * found at start-up are the baseline: a reload notifies nothing.
 */

/** How long a wait must last before it is signalled; also the window of a burst. */
export const SETTLE_MS = 2_000;
/** How long after a signal the same session is not signalled again. */
export const COOLDOWN_MS = 30_000;

type SignalKind = "waiting" | "done";

export interface MonitorDeps {
  workspace: WorkspaceStore;
  claude: ClaudeIntegrationStore;
  prefs: NotificationPrefs;
  /** Whether the window has the focus. */
  windowFocused: () => boolean;
  /** Says something to screen readers, politely. */
  announce: (text: string) => void;
  now?: () => number;
}

export class AttentionMonitor {
  private readonly now: () => number;
  private baseline = false;
  /** The current wait of each session that waits. */
  private waits = new Map<SessionId, string>();
  /** The status of the agent of each session that reports one. */
  private statuses = new Map<SessionId, string>();
  private readonly pending = new Map<SessionId, SignalKind>();
  private timer: ReturnType<typeof setTimeout> | null = null;
  private readonly signalled = new Map<SessionId, number>();
  private badge: number | null = null;
  private stops: (() => void)[] = [];

  constructor(private readonly deps: MonitorDeps) {
    this.now = deps.now ?? Date.now;
  }

  /** Starts watching. Returns how to stop. */
  start(): () => void {
    if (this.stops.length === 0) {
      this.stops = [
        this.deps.workspace.subscribe(this.evaluate),
        this.deps.claude.subscribe(this.evaluate),
      ];
      this.evaluate();
    }
    return this.stop;
  }

  stop = (): void => {
    for (const stop of this.stops) stop();
    this.stops = [];
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
  };

  private evaluate = (): void => {
    const workspace = this.deps.workspace.workspace;
    const claude = this.deps.claude;
    if (!workspace || claude.getSnapshot().state.status === "loading") return;

    const sessions = claude.reporting ? workspace.sessions : [];
    const waits = new Map<SessionId, string>();
    const statuses = new Map<SessionId, string>();
    for (const session of sessions) {
      if (isWaiting(session)) waits.set(session.id, waitKey(session));
      const view = viewAgent(session.agent);
      if (view && !view.stale) statuses.set(session.id, view.icon);
    }

    if (this.baseline) {
      for (const [id, key] of waits) {
        if (this.waits.get(id) !== key) this.enter(id, "waiting");
      }
      for (const [id, status] of statuses) {
        const before = this.statuses.get(id);
        if (status === "done" && before !== undefined && before !== "done") this.enter(id, "done");
      }
      // What ended before it settled is forgotten.
      for (const [id, kind] of [...this.pending]) {
        const still = kind === "waiting" ? waits.has(id) : statuses.get(id) === "done";
        if (!still) this.pending.delete(id);
      }
    }
    this.baseline = true;
    this.waits = waits;
    this.statuses = statuses;
    this.syncBadge(waits.size);
  };

  private enter(id: SessionId, kind: SignalKind): void {
    const last = this.signalled.get(id);
    if (last !== undefined && this.now() - last < COOLDOWN_MS) return;
    // Waiting wins over done.
    if (this.pending.get(id) !== "waiting") this.pending.set(id, kind);
    this.timer ??= setTimeout(this.flush, SETTLE_MS);
  }

  private flush = (): void => {
    this.timer = null;
    const workspace = this.deps.workspace.workspace;
    const pending = [...this.pending];
    this.pending.clear();
    if (!workspace || !this.deps.claude.reporting) return;

    const byId = new Map(workspace.sessions.map((session) => [session.id, session]));
    const waiting: Session[] = [];
    const done: Session[] = [];
    for (const [id, kind] of pending) {
      const session = byId.get(id);
      if (!session) continue;
      if (kind === "waiting" && isWaiting(session)) waiting.push(session);
      else if (kind === "done" && viewAgent(session.agent)?.icon === "done") done.push(session);
    }
    if (waiting.length === 0 && done.length === 0) return;

    // The oldest wait first, as in the queue.
    const order = new Map(attentionQueue(workspace).map((session, index) => [session.id, index]));
    waiting.sort((a, b) => (order.get(a.id) ?? 0) - (order.get(b.id) ?? 0));

    const at = this.now();
    for (const session of [...waiting, ...done]) this.signalled.set(session.id, at);
    this.deps.announce(announcement(waiting, done));

    const settings = this.deps.prefs.get();
    const looking = this.lookingAt(workspace);
    const notify = (session: Session, kind: SignalKind) =>
      !looking.has(session.id) && (kind === "waiting" ? settings.waiting : settings.done);
    const notification = describeNotification(
      waiting.filter((session) => notify(session, "waiting")),
      done.filter((session) => notify(session, "done")),
    );
    if (notification && this.deps.claude.available) {
      this.deps.claude.backend.notify(notification).catch(ignoreFailure);
    }
  };

  /** The sessions the user sees now: on screen, in the focused window. */
  private lookingAt(workspace: WorkspaceSnapshot): Set<SessionId> {
    return this.deps.windowFocused() ? visibleSessions(workspace.layout) : new Set();
  }

  private syncBadge(count: number): void {
    if (!this.deps.claude.available || count === this.badge) return;
    this.badge = count;
    this.deps.claude.backend.setAttentionBadge(count).catch(ignoreFailure);
  }
}

/** A list of titles for a sentence: "a, b and 2 more". */
function titles(sessions: readonly Session[]): string {
  const names = sessions.map((session) => session.title);
  if (names.length <= 3) {
    return names.length <= 1
      ? (names[0] ?? "")
      : `${names.slice(0, -1).join(", ")} and ${names.at(-1) ?? ""}`;
  }
  return `${names.slice(0, 3).join(", ")} and ${String(names.length - 3)} more`;
}

function waitLabel(session: Session): string {
  return viewAgent(session.agent)?.longLabel ?? "Needs you";
}

/** The OS notification for these sessions, or `null` for none. */
export function describeNotification(
  waiting: readonly Session[],
  done: readonly Session[],
): { title: string; body: string; sessionId: SessionId } | null {
  const first = waiting[0] ?? done[0];
  if (!first) return null;
  const doneNote =
    done.length === 0
      ? ""
      : ` · ${done.length === 1 ? "1 session" : `${String(done.length)} sessions`} done`;
  if (waiting.length === 1) {
    return {
      title: first.title,
      body: `${waitLabel(first)} · ${first.project.name}${doneNote}`,
      sessionId: first.id,
    };
  }
  if (waiting.length > 1) {
    return {
      title: `${String(waiting.length)} sessions need you`,
      body: `${titles(waiting)}${doneNote}`,
      sessionId: first.id,
    };
  }
  if (done.length === 1) {
    return { title: first.title, body: `Done · ${first.project.name}`, sessionId: first.id };
  }
  return {
    title: `${String(done.length)} sessions are done`,
    body: titles(done),
    sessionId: first.id,
  };
}

/** What screen readers hear about these sessions. */
export function announcement(waiting: readonly Session[], done: readonly Session[]): string {
  const parts: string[] = [];
  const [one] = waiting;
  if (waiting.length === 1 && one) {
    parts.push(`${one.title} needs you: ${waitLabel(one).toLowerCase()}.`);
  } else if (waiting.length > 1) {
    parts.push(`${String(waiting.length)} sessions need you: ${titles(waiting)}.`);
  }
  if (done.length === 1 && done[0]) parts.push(`${done[0].title} is done.`);
  else if (done.length > 1)
    parts.push(`${String(done.length)} sessions are done: ${titles(done)}.`);
  return parts.join(" ");
}

/** A failed call was logged where it failed; nothing else to do. */
function ignoreFailure(): void {
  // Nothing to undo.
}
