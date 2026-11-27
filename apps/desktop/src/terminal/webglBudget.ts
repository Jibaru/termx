/**
 * How many WebGL contexts the terminals of a window hold.
 *
 * A webview allows a handful of WebGL contexts (16 in Chromium and WebKit);
 * past that it drops the oldest one, which may well belong to a pane on
 * screen. So the terminals share a budget below that limit: a terminal
 * keeps its context while it is shown, and also after it leaves the screen
 * so that switching back is instant, until a terminal that is shown needs
 * one and the budget is spent. The terminal off the screen that was shown
 * least recently then gives its context back (explicitly, with
 * `WEBGL_lose_context`, rather than waiting for the garbage collector) and
 * draws with the DOM renderer until it is shown again.
 */

/** Contexts the terminals of a window may hold at once. */
export const MAX_WEBGL_CONTEXTS = 8;

/** What the budget needs from a terminal. */
export interface WebglHolder {
  /** Gives the context back. The terminal may ask again when it is shown. */
  evictWebgl(): void;
}

export class WebglBudget {
  /** Holders of a context, least recently shown first. */
  private readonly holders: WebglHolder[] = [];
  private readonly visible = new Set<WebglHolder>();

  constructor(readonly limit: number = MAX_WEBGL_CONTEXTS) {}

  /**
   * Asks for a context for a terminal that is shown. Makes room by
   * evicting the terminal off the screen that was shown least recently.
   * Returns `false` when every context belongs to a terminal on screen.
   */
  acquire(holder: WebglHolder): boolean {
    if (this.holders.includes(holder)) {
      this.shown(holder);
      return true;
    }
    while (this.holders.length >= this.limit) {
      const evictable = this.holders.find((candidate) => !this.visible.has(candidate));
      if (!evictable) return false;
      this.release(evictable);
      evictable.evictWebgl();
    }
    this.holders.push(holder);
    this.visible.add(holder);
    return true;
  }

  /** The terminal is on screen: it is the last to be evicted. */
  shown(holder: WebglHolder): void {
    const index = this.holders.indexOf(holder);
    if (index === -1) return;
    this.holders.splice(index, 1);
    this.holders.push(holder);
    this.visible.add(holder);
  }

  /** The terminal left the screen: it keeps its context, for now. */
  hidden(holder: WebglHolder): void {
    this.visible.delete(holder);
  }

  /** The terminal gave its context back, or lost it. */
  release(holder: WebglHolder): void {
    const index = this.holders.indexOf(holder);
    if (index !== -1) this.holders.splice(index, 1);
    this.visible.delete(holder);
  }

  /** Forgets every holder, without evicting any. For tests. */
  clear(): void {
    this.holders.length = 0;
    this.visible.clear();
  }

  /** How many contexts are held. */
  get size(): number {
    return this.holders.length;
  }
}

/** The budget of this window. */
export const webglBudget = new WebglBudget();

/**
 * Loses the WebGL contexts of `canvases` at once, so that the webview can
 * count them as gone.
 */
export function loseContexts(canvases: readonly HTMLCanvasElement[]): void {
  for (const canvas of canvases) {
    try {
      const context = canvas.getContext("webgl2");
      context?.getExtension("WEBGL_lose_context")?.loseContext();
    } catch {
      // A canvas without a WebGL context: nothing to give back.
    }
  }
}
