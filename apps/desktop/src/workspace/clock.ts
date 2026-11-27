import { useSyncExternalStore } from "react";

/**
 * One clock for every "5m ago" of the window: a single timer, running only
 * while something shows a relative time.
 */

export const CLOCK_TICK_MS = 15_000;

let now = Date.now();
const listeners = new Set<() => void>();
let timer: ReturnType<typeof setInterval> | null = null;

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  if (timer === null) {
    now = Date.now();
    timer = setInterval(() => {
      now = Date.now();
      for (const each of listeners) each();
    }, CLOCK_TICK_MS);
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0 && timer !== null) {
      clearInterval(timer);
      timer = null;
    }
  };
}

function getNow(): number {
  return now;
}

/** The time, updated every `CLOCK_TICK_MS`. */
export function useNow(): number {
  return useSyncExternalStore(subscribe, getNow);
}
