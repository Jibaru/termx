import type { ClaudeIntegrationStore } from "../claude/integration";
import type { AttentionNotification, Session, SessionId, WorkspaceSnapshot } from "../lib/ipc";
import type { FocusSource } from "../lib/windowFocus";
import { visibleSessions } from "../workspace/layoutTree";
import type { WorkspaceStore } from "../workspace/workspaceStore";
import { attentionQueue, isWaiting, viewAgent, waitKey } from "./agent";
import type { NotificationPrefs } from "./notificationPrefs";

/**
 * Tells the user when a session starts to need them, without crying wolf
 * and without losing a wait.
 *
 * Each wait of a session (identified by its `waitingSince`) is followed
 * until it ends:
 *
 * - **It settles first.** A wait is signalled only once it lasted
 *   `SETTLE_MS`: a permission granted at once by a rule never shows. Every
 *   wait of a burst gets that time, not only the first.
 * - **Bursts are one notification.** Waits that start within `SETTLE_MS` of
 *   the first of a burst go out together, once the last of them settled:
 *   "3 sessions need you".
 * - **Not while the user looks at it.** No OS notification for a session
 *   shown in a pane of the focused, visible window. The wait is kept: if
 *   the user looks away while it still waits, it is notified then.
 * - **No repeat within `COOLDOWN_MS`.** After an OS notification about a
 *   session, a new wait of the same session is held, not dropped, and
 *   notified when the cooldown ends, if it still waits. The cooldown only
 *   starts when a notification was actually sent.
 * - **Once per wait.** A wait is notified at most once; with notifications
 *   turned off when it settles, it is not notified later.
 * - "Done" follows the same rules when the user turned it on.
 *
 * Every wait that settles is also announced to screen readers (`announce`),
 * and the badge of the app always holds the number of sessions that wait.
 *
 * Nothing is sent before the plugin reports (`reporting`), and the waits
 * found at start-up are the baseline: a reload notifies nothing.
 */

/** How long a wait must last before it is signalled; also the window of a burst. */
export const SETTLE_MS = 2_000;
/** How long after a notification about a session no other one is sent about it. */
export const COOLDOWN_MS = 30_000;

type SignalKind = "waiting" | "done";

/** One wait (or one "done") of a session, followed until it ends. */
interface Signal {
  id: SessionId;
  kind: SignalKind;
  /** Identifies the wait: a new one has a new key. */
  key: string;
  /** When termx saw it start. */
  seen: number;
  announced: boolean;
  /** Notified, or never to be: it settled while notifications were off. */
  settledNotification: boolean;
}

export interface MonitorDeps {
  workspace: WorkspaceStore;
  claude: ClaudeIntegrationStore;
  prefs: NotificationPrefs;
  /** Whether the user can be looking at the window. */
  focus: FocusSource;
  /** Says something to screen readers, politely. */
  announce: (text: string) => void;
  now?: () => number;
}

export class AttentionMonitor {
  private readonly now: () => number;
  private baseline = false;
  /** The signal being followed for each session, if any. */
  private signals = new Map<SessionId, Signal>();
  /** The status of the agent of each session that reports one. */
  private statuses = new Map<SessionId, string>();
  /** When the last OS notification about each session was sent. */
  private readonly notifiedAt = new Map<SessionId, number>();
  private timer: ReturnType<typeof setTimeout> | null = null;
  private timerAt: number | null = null;
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
        this.deps.prefs.subscribe(this.review),
        // Looking away from a session that waits can make it due.
        this.deps.focus.subscribe(this.review),
      ];
      this.evaluate();
    }
    return this.stop;
  }

  stop = (): void => {
    for (const stop of this.stops) stop();
    this.stops = [];
    this.clearTimer();
  };

  /** Follows what the workspace says now: waits that start and end. */
  private evaluate = (): void => {
    const workspace = this.deps.workspace.workspace;
    const claude = this.deps.claude;
    if (!workspace || claude.getSnapshot().state.status === "loading") return;

    const sessions = claude.reporting ? workspace.sessions : [];
    const statuses = new Map<SessionId, string>();
    const signals = new Map<SessionId, Signal>();
    const now = this.now();
    let waiting = 0;
    for (const session of sessions) {
      const view = viewAgent(session.agent);
      if (view && !view.stale) statuses.set(session.id, view.icon);
      const previous = this.signals.get(session.id);
      let key: string | null = null;
      let kind: SignalKind = "waiting";
      if (isWaiting(session)) {
        waiting += 1;
        key = waitKey(session);
      } else if (view?.icon === "done" && !view.stale) {
        const before = this.statuses.get(session.id);
        kind = "done";
        // A session that just finished, or one followed since it did.
        if (previous?.kind === "done") key = previous.key;
        else if (before !== undefined && before !== "done")
          key = `${session.id}\u0000done:${String(now)}`;
      }
      if (key === null) continue;
      if (previous?.key === key) {
        signals.set(session.id, previous);
      } else {
        signals.set(session.id, {
          id: session.id,
          kind,
          key,
          seen: now,
          // The waits found at start-up are never signalled.
          announced: !this.baseline,
          settledNotification: !this.baseline,
        });
      }
    }
    this.baseline = true;
    this.signals = signals;
    this.statuses = statuses;
    this.syncBadge(waiting);
    this.review();
  };

  /** Sends what is due, and sets a timer for what will be. */
  private review = (): void => {
    const workspace = this.deps.workspace.workspace;
    if (!workspace || !this.deps.claude.reporting) return;
    const now = this.now();
    let next: number | null = null;
    const later = (at: number) => {
      next = next === null ? at : Math.min(next, at);
    };

    // Screen readers hear every wait once it settled.
    const toAnnounce = this.due(
      [...this.signals.values()].filter((signal) => !signal.announced),
      now,
      later,
    );
    if (toAnnounce.length > 0) {
      for (const signal of toAnnounce) signal.announced = true;
      const [waiting, done] = this.sessionsOf(workspace, toAnnounce);
      this.deps.announce(announcement(waiting, done));
    }

    // OS notifications.
    const settings = this.deps.prefs.get();
    const looking = this.deps.focus.focused() ? visibleSessions(workspace.layout) : new Set();
    const candidates: Signal[] = [];
    for (const signal of this.signals.values()) {
      if (signal.settledNotification) continue;
      const settled = now >= signal.seen + SETTLE_MS;
      const wanted = signal.kind === "waiting" ? settings.waiting : settings.done;
      if (!wanted) {
        // Off when it settles: never notified.
        if (settled) signal.settledNotification = true;
        else later(signal.seen + SETTLE_MS);
        continue;
      }
      // Kept for when the user looks away.
      if (looking.has(signal.id)) {
        if (!settled) later(signal.seen + SETTLE_MS);
        continue;
      }
      const last = this.notifiedAt.get(signal.id);
      if (last !== undefined && now < last + COOLDOWN_MS) {
        later(last + COOLDOWN_MS);
        continue;
      }
      candidates.push(signal);
    }
    const toNotify = this.due(candidates, now, later);
    if (toNotify.length > 0) {
      const [waiting, done] = this.sessionsOf(workspace, toNotify);
      const notification = describeNotification(waiting, done);
      if (notification && this.deps.claude.available) {
        for (const signal of toNotify) {
          signal.settledNotification = true;
          this.notifiedAt.set(signal.id, now);
        }
        this.deps.claude.backend.notify(notification).catch(ignoreFailure);
      }
    }

    this.schedule(next);
  };

  /**
   * The signals of `candidates` that are due now: the burst of the oldest
   * one (those seen within `SETTLE_MS` of it), once the last of the burst
   * settled. Tells `later` when the rest will be due.
   */
  private due(candidates: Signal[], now: number, later: (at: number) => void): Signal[] {
    const result: Signal[] = [];
    let rest = [...candidates].sort((a, b) => a.seen - b.seen);
    while (rest.length > 0) {
      const first = rest[0];
      if (!first) break;
      const burst = rest.filter((signal) => signal.seen <= first.seen + SETTLE_MS);
      const dueAt = Math.max(...burst.map((signal) => signal.seen)) + SETTLE_MS;
      if (now < dueAt) {
        later(dueAt);
        break;
      }
      result.push(...burst);
      rest = rest.filter((signal) => !burst.includes(signal));
    }
    return result;
  }

  private sessionsOf(workspace: WorkspaceSnapshot, signals: Signal[]): [Session[], Session[]] {
    const byId = new Map(workspace.sessions.map((session) => [session.id, session]));
    const order = new Map(attentionQueue(workspace).map((session, index) => [session.id, index]));
    const waiting: Session[] = [];
    const done: Session[] = [];
    for (const signal of signals) {
      const session = byId.get(signal.id);
      if (!session) continue;
      (signal.kind === "waiting" ? waiting : done).push(session);
    }
    // The oldest wait first, as in the queue.
    waiting.sort((a, b) => (order.get(a.id) ?? 0) - (order.get(b.id) ?? 0));
    return [waiting, done];
  }

  private schedule(at: number | null): void {
    if (at === this.timerAt) return;
    this.clearTimer();
    if (at === null) return;
    this.timerAt = at;
    this.timer = setTimeout(
      () => {
        this.timer = null;
        this.timerAt = null;
        this.review();
      },
      Math.max(0, at - this.now()),
    );
  }

  private clearTimer(): void {
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
    this.timerAt = null;
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
): AttentionNotification | null {
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
