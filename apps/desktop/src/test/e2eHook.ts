import type { SessionStore } from "../terminal/sessionStore";

/**
 * The hook of the end-to-end suite (`e2e/` at the repository root), which
 * drives the real application and reads the terminals through xterm.js.
 *
 * Builds made for that suite set `window.__TERMX_E2E__` to `true` before the
 * page runs (`src-tauri/src/e2e.rs`, compiled only with the `e2e` cargo
 * feature, which a release build refuses). Only then are the terminals of
 * the window exposed, on that same property. Everywhere else this does
 * nothing.
 */

/** What the end-to-end suite reaches the terminals through. */
export interface E2eHook {
  store: SessionStore;
}

declare global {
  interface Window {
    __TERMX_E2E__?: true | E2eHook;
  }
}

export function installE2eHook(store: SessionStore, target: Window = window): void {
  if (target.__TERMX_E2E__ !== true) return;
  target.__TERMX_E2E__ = { store };
}
