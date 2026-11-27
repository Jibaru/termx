import { isBoolean, readPref, writePref } from "../lib/prefs";

/**
 * When termx sends an OS notification about a session: when it needs the
 * user (on by default), and when it is done (off by default). Kept with the
 * other preferences of the window until `config.toml` holds them.
 */
export interface NotificationSettings {
  /** "Notify me when a session needs me". */
  waiting: boolean;
  /** Also when a session is done. */
  done: boolean;
}

const WAITING_PREF = "notifyWhenWaiting";
const DONE_PREF = "notifyWhenDone";

export class NotificationPrefs {
  private settings: NotificationSettings = {
    waiting: readPref(WAITING_PREF, true, isBoolean),
    done: readPref(DONE_PREF, false, isBoolean),
  };
  private readonly listeners = new Set<() => void>();

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  get = (): NotificationSettings => this.settings;

  set = (change: Partial<NotificationSettings>): void => {
    this.settings = { ...this.settings, ...change };
    writePref(WAITING_PREF, this.settings.waiting);
    writePref(DONE_PREF, this.settings.done);
    for (const listener of this.listeners) listener();
  };
}
