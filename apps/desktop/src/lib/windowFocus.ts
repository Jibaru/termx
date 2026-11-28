import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * Whether the user can be looking at the window: it has the focus of the
 * system and the page is visible (not minimised, not on a hidden space).
 */
export interface FocusSource {
  focused(): boolean;
  /** Calls `listener` whenever `focused()` changes. Returns how to stop. */
  subscribe(listener: () => void): () => void;
}

/**
 * The focus of this window, from the focus and blur events of Tauri and
 * `document.visibilityState`. Outside Tauri (tests, a plain browser) the
 * focus and blur events of the page stand in for those of Tauri. Starts
 * listening on the first subscription.
 */
export function windowFocusSource(): FocusSource {
  const listeners = new Set<() => void>();
  // Until Tauri answers: a window that was just opened has the focus.
  let windowFocused = true;
  let current = compute();
  let started = false;

  function compute(): boolean {
    return windowFocused && document.visibilityState !== "hidden";
  }

  function update(): void {
    const next = compute();
    if (next === current) return;
    current = next;
    for (const listener of listeners) listener();
  }

  function setWindowFocused(value: boolean): void {
    windowFocused = value;
    update();
  }

  function start(): void {
    started = true;
    document.addEventListener("visibilitychange", update);
    try {
      const window = getCurrentWindow();
      window.isFocused().then(setWindowFocused, ignoreFailure);
      window
        .onFocusChanged(({ payload }) => {
          setWindowFocused(payload);
        })
        .catch(ignoreFailure);
    } catch {
      // Not in Tauri: the page's own events.
      globalThis.addEventListener("focus", () => {
        setWindowFocused(true);
      });
      globalThis.addEventListener("blur", () => {
        setWindowFocused(false);
      });
    }
  }

  return {
    focused: () => current,
    subscribe(listener) {
      if (!started) start();
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}

/** A focus source that is always focused unless told otherwise. For tests. */
export function manualFocusSource(initial = true): FocusSource & { set(value: boolean): void } {
  let focused = initial;
  const listeners = new Set<() => void>();
  return {
    focused: () => focused,
    subscribe(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    set(value) {
      if (value === focused) return;
      focused = value;
      for (const listener of listeners) listener();
    },
  };
}

function ignoreFailure(): void {
  // Logged where it failed; the last known focus stays.
}
