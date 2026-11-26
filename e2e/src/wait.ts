/** How long a condition is waited for by default, in milliseconds. */
export const DEFAULT_TIMEOUT_MS = 30_000;

/** How often a condition is checked, in milliseconds. */
const POLL_INTERVAL_MS = 50;

export interface WaitOptions {
  /** What is being waited for, for the message of the failure. */
  what: string;
  timeoutMs?: number;
  /** Described in the failure, to show how far things got. */
  describe?: () => unknown;
}

export function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * Checks `condition` until it returns something other than `undefined`,
 * `null` or `false`, and returns that. Fails once the time is up.
 *
 * This is the only way a test waits: on a condition, never for a fixed time.
 */
export async function waitFor<T>(
  condition: () => T | Promise<T>,
  options: WaitOptions,
): Promise<NonNullable<T>> {
  const timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await condition();
    if (value !== undefined && value !== null && value !== false) return value;
    if (Date.now() >= deadline) {
      let detail = "";
      if (options.describe) {
        const described = await Promise.resolve()
          .then(options.describe)
          .catch((error: unknown) => `(could not be described: ${String(error)})`);
        detail = `\n${typeof described === "string" ? described : JSON.stringify(described, null, 2)}`;
      }
      throw new Error(
        `Timed out after ${String(timeoutMs)} ms waiting for ${options.what}${detail}`,
      );
    }
    await delay(POLL_INTERVAL_MS);
  }
}
