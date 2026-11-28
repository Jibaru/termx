import type { ClaudeBackend } from "../claude/backend";
import type { ClaudeIntegration, ClaudeIntegrationPreview, SessionId } from "../lib/ipc";

/**
 * The Claude Code integration on the Rust side, as far as the interface can
 * tell: it records every command, keeps the state the commands change, and
 * lets a test click a notification. Responders replace what a command does
 * (throw a `CommandError` from one to make it fail).
 */

export interface ClaudeCall {
  command: keyof ClaudeBackend;
  args: unknown[];
}

type Responders = Partial<{
  [K in keyof ClaudeBackend]: (...args: Parameters<ClaudeBackend[K]>) => unknown;
}>;

export const INSTALLED: ClaudeIntegration = {
  claudeInstalled: true,
  plugin: "installed",
  version: "1.0.0",
  consent: "accepted",
};

export const PREVIEW: ClaudeIntegrationPreview = {
  changes: [
    {
      path: "~/.claude/plugins/termx",
      description: "The plugin of termx: hooks that report what each session does.",
    },
    {
      path: "~/.claude/settings.json",
      description: "Enables the plugin. Nothing else in the file changes.",
    },
  ],
};

export function fakeClaude(initial: ClaudeIntegration = INSTALLED, responders: Responders = {}) {
  const calls: ClaudeCall[] = [];
  let state: ClaudeIntegration = { ...initial };
  let clicked: ((id: SessionId) => void) | null = null;
  let changed: (() => void) | null = null;

  const answer = (command: keyof ClaudeBackend, args: unknown[], fallback: () => unknown) => {
    calls.push({ command, args });
    const responder = responders[command] as ((...a: unknown[]) => unknown) | undefined;
    try {
      return Promise.resolve((responder ? responder(...args) : fallback()) as never);
    } catch (error) {
      // eslint-disable-next-line @typescript-eslint/prefer-promise-reject-errors
      return Promise.reject(error);
    }
  };

  const backend: ClaudeBackend = {
    getClaudeIntegration: () => answer("getClaudeIntegration", [], () => ({ ...state })),
    previewClaudeIntegration: () => answer("previewClaudeIntegration", [], () => PREVIEW),
    installClaudeIntegration: () =>
      answer("installClaudeIntegration", [], () => {
        state = { ...state, plugin: "installed", version: "1.1.0" };
        delete state.error;
      }),
    uninstallClaudeIntegration: () =>
      answer("uninstallClaudeIntegration", [], () => {
        state = { ...state, plugin: "notInstalled" };
        delete state.version;
      }),
    setClaudeConsent: (consent) =>
      answer("setClaudeConsent", [consent], () => {
        state = { ...state, consent };
      }),
    notify: (notification) => answer("notify", [notification], () => undefined),
    // Whether the platform shows a badge.
    setAttentionBadge: (count) => answer("setAttentionBadge", [count], () => true),
    onClaudeIntegrationChanged: (handler) => {
      changed = handler;
      return Promise.resolve(() => {
        changed = null;
      });
    },
    onNotificationClicked: (handler) => {
      clicked = handler;
      return Promise.resolve(() => {
        clicked = null;
      });
    },
  };

  const callsOf = (command: keyof ClaudeBackend) =>
    calls.filter((call) => call.command === command).map((call) => call.args);

  /** The user clicks a notification about `id`. */
  const click = (id: SessionId) => {
    if (!clicked) throw new Error("nobody listens to notification clicks");
    clicked(id);
  };

  /** The Rust side says the integration changed. */
  const announceChange = () => {
    if (!changed) throw new Error("nobody listens to changes of the integration");
    changed();
  };

  return {
    backend,
    announceChange,
    calls,
    callsOf,
    click,
    /** Changes what the Rust side says, as if Claude Code had changed. */
    setState: (change: Partial<ClaudeIntegration>) => {
      state = { ...state, ...change };
    },
  };
}
