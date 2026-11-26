/**
 * Bounds how many records the frontend sends to the log, so that an error
 * that repeats on every frame does not flood the IPC channel. The Rust side
 * applies its own limit as well; this one keeps the calls from being made at
 * all.
 *
 * - Every level has its own budget per time window, so low-level noise
 *   cannot use up the room for errors.
 * - An error that was not seen before draws on a second budget when repeats
 *   used up the error budget, so the first occurrence is not lost. Errors are
 *   the same when they have the same source and the same message once
 *   numbers and identifiers are ignored, so numbering the messages does not
 *   make each of them new.
 * - That second budget is per source, so a source that produces an endless
 *   stream of different errors only exhausts its own.
 * - Every dropped record is counted, and the counts are handed out with the
 *   next admitted record so that they can be logged.
 */

export type LimitedLevel = "error" | "warn" | "info";

export type DroppedCounts = Record<LimitedLevel, number>;

export interface LogLimits {
  /** Records of each level allowed per window. */
  perLevel: DroppedCounts;
  /** Extra errors each source is allowed per window when they are new. */
  newErrorsPerSource: number;
  /** Length of a window in milliseconds. */
  windowMs: number;
}

export const DEFAULT_LOG_LIMITS: LogLimits = {
  perLevel: { error: 20, warn: 10, info: 10 },
  newErrorsPerSource: 10,
  windowMs: 10_000,
};

/** Distinct errors remembered before the memory starts over. */
const MAX_REMEMBERED_ERRORS = 500;

/** Sources that get a budget of their own. Any others share one. */
const MAX_SOURCES = 32;
const OTHER_SOURCES = "\u0000others";

/** Longest message key kept. */
const MAX_KEY_LENGTH = 160;

/**
 * Reduces a message to what identifies the problem: every word that contains
 * a digit (numbers, identifiers, addresses, hashes) becomes `#`, groups of
 * them joined by punctuation become one `#`, and letter case and the amount
 * of white space are ignored. Mirrors `message_key` on the Rust side.
 */
export function messageKey(message: string): string {
  return message
    .toLowerCase()
    .replace(/[\p{L}\p{N}_]*\p{Nd}[\p{L}\p{N}_]*/gu, "#")
    .replace(/#(?:[-.:,/x]#)+/g, "#")
    .replace(/#+/g, "#")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, MAX_KEY_LENGTH);
}

function noCounts(): DroppedCounts {
  return { error: 0, warn: 0, info: 0 };
}

export class LogLimiter {
  private windowStart = Number.NEGATIVE_INFINITY;
  private used = noCounts();
  private usedNewErrors = new Map<string, number>();
  private dropped = noCounts();
  private readonly seenErrors = new Set<string>();

  constructor(
    private readonly limits: LogLimits = DEFAULT_LOG_LIMITS,
    private readonly now: () => number = Date.now,
  ) {}

  /**
   * Asks whether a record may be sent. `record` is the source of the record,
   * a line feed, and its message; together they tell a new error from a
   * repeat.
   *
   * Returns `null` when the record must be dropped. Otherwise returns the
   * records dropped since they were last handed out (usually all zero), which
   * the caller should log.
   */
  admit(level: LimitedLevel, record: string): DroppedCounts | null {
    const lineBreak = record.indexOf("\n");
    const source = lineBreak < 0 ? "" : record.slice(0, lineBreak);
    const message = record.slice(lineBreak + 1);

    const now = this.now();
    if (now - this.windowStart >= this.limits.windowMs) {
      this.windowStart = now;
      this.used = noCounts();
      this.usedNewErrors = new Map();
    }

    let admitted = false;
    if (level === "error") {
      if (this.seenErrors.size >= MAX_REMEMBERED_ERRORS) this.seenErrors.clear();
      const key = `${source}\n${messageKey(message)}`;
      const isNew = !this.seenErrors.has(key);
      this.seenErrors.add(key);
      if (this.used.error < this.limits.perLevel.error) {
        this.used.error += 1;
        admitted = true;
      } else if (isNew) {
        const budget =
          this.usedNewErrors.has(source) || this.usedNewErrors.size < MAX_SOURCES
            ? source
            : OTHER_SOURCES;
        const used = this.usedNewErrors.get(budget) ?? 0;
        if (used < this.limits.newErrorsPerSource) {
          this.usedNewErrors.set(budget, used + 1);
          admitted = true;
        }
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
