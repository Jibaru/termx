/**
 * Bounds how many records the frontend sends to the log, so that an error
 * that repeats on every frame does not flood the IPC channel. The Rust side
 * applies its own limit as well; this one keeps the calls from being made at
 * all.
 *
 * - Every level has its own budget per time window, so low-level noise
 *   cannot use up the room for errors.
 * - An error that was not seen before draws on a second budget when repeats
 *   used up the error budget, so the first occurrence is not lost.
 * - Every dropped record is counted, and the counts are handed out with the
 *   next admitted record so that they can be logged.
 */

export type LimitedLevel = "error" | "warn" | "info";

export type DroppedCounts = Record<LimitedLevel, number>;

export interface LogLimits {
  /** Records of each level allowed per window. */
  perLevel: DroppedCounts;
  /** Extra errors allowed per window when they were not seen before. */
  newErrors: number;
  /** Length of a window in milliseconds. */
  windowMs: number;
}

export const DEFAULT_LOG_LIMITS: LogLimits = {
  perLevel: { error: 20, warn: 10, info: 10 },
  newErrors: 30,
  windowMs: 10_000,
};

/** Distinct errors remembered before the memory starts over. */
const MAX_REMEMBERED_ERRORS = 500;

function noCounts(): DroppedCounts {
  return { error: 0, warn: 0, info: 0 };
}

export class LogLimiter {
  private windowStart = Number.NEGATIVE_INFINITY;
  private used = noCounts();
  private usedNewErrors = 0;
  private dropped = noCounts();
  private readonly seenErrors = new Set<string>();

  constructor(
    private readonly limits: LogLimits = DEFAULT_LOG_LIMITS,
    private readonly now: () => number = Date.now,
  ) {}

  /**
   * Asks whether a record may be sent. `key` tells a new error from a repeat.
   *
   * Returns `null` when the record must be dropped. Otherwise returns the
   * records dropped since they were last handed out (usually all zero), which
   * the caller should log.
   */
  admit(level: LimitedLevel, key: string): DroppedCounts | null {
    const now = this.now();
    if (now - this.windowStart >= this.limits.windowMs) {
      this.windowStart = now;
      this.used = noCounts();
      this.usedNewErrors = 0;
    }

    let admitted = false;
    if (level === "error") {
      if (this.seenErrors.size >= MAX_REMEMBERED_ERRORS) this.seenErrors.clear();
      const isNew = !this.seenErrors.has(key);
      this.seenErrors.add(key);
      if (this.used.error < this.limits.perLevel.error) {
        this.used.error += 1;
        admitted = true;
      } else if (isNew && this.usedNewErrors < this.limits.newErrors) {
        this.usedNewErrors += 1;
        admitted = true;
      }
    } else if (this.used[level] < this.limits.perLevel[level]) {
      this.used[level] += 1;
      admitted = true;
    }

    if (!admitted) {
      this.dropped[level] += 1;
      return null;
    }
    const dropped = this.dropped;
    this.dropped = noCounts();
    return dropped;
  }
}
