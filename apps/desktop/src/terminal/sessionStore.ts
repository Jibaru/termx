import { TerminalController } from "./TerminalController";

/**
 * The terminals of the window, keyed by an identifier that stays the same
 * for the whole life of a terminal.
 *
 * That identifier is not the one of the session in the Rust core: a
 * terminal whose process exited gets a new session when it is restarted,
 * and stays the same terminal, with the same scrollback.
 *
 * Only the active terminal is on screen. The others keep running: their
 * controllers hold the xterm.js instance and keep feeding it.
 */

/** What the interface renders from. A new object after every change. */
export interface SessionsSnapshot {
  /** The terminals, in the order they were opened. */
  readonly keys: readonly string[];
  /** The terminal on screen, if there is one. */
  readonly activeKey: string | null;
}

export class SessionStore {
  private readonly controllers = new Map<string, TerminalController>();
  private snapshot: SessionsSnapshot = { keys: [], activeKey: null };
  private readonly listeners = new Set<() => void>();
  private nextKey = 1;

  /** `createController` builds the terminal of a new entry. Tests replace it. */
  constructor(
    private readonly createController: () => TerminalController = () => new TerminalController(),
  ) {}

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getSnapshot = (): SessionsSnapshot => this.snapshot;

  /** The terminal with this key. */
  get(key: string): TerminalController | undefined {
    return this.controllers.get(key);
  }

  /** Opens a terminal, makes it the active one and returns its key. */
  open(): string {
    const key = `terminal-${String(this.nextKey++)}`;
    this.controllers.set(key, this.createController());
    this.update({ keys: [...this.snapshot.keys, key], activeKey: key });
    return key;
  }

  /** Opens the first terminal, unless there is one already. */
  ensureOne(): void {
    if (this.controllers.size === 0) this.open();
  }

  /** Puts another terminal on screen. */
  activate(key: string): void {
    if (!this.controllers.has(key) || key === this.snapshot.activeKey) return;
    this.update({ ...this.snapshot, activeKey: key });
  }

  /** Closes a terminal and its session. A neighbour becomes active. */
  close(key: string): void {
    const controller = this.controllers.get(key);
    if (!controller) return;
    this.controllers.delete(key);
    controller.dispose();

    const index = this.snapshot.keys.indexOf(key);
    const keys = this.snapshot.keys.filter((other) => other !== key);
    const activeKey =
      this.snapshot.activeKey === key
        ? (keys[Math.min(index, keys.length - 1)] ?? null)
        : this.snapshot.activeKey;
    this.update({ keys, activeKey });
  }

  private update(snapshot: SessionsSnapshot): void {
    this.snapshot = snapshot;
    for (const listener of this.listeners) listener();
  }
}

/** The terminals of this window. */
export const sessionStore = new SessionStore();
