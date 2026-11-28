import { reportError } from "../lib/errorReporting";
import {
  type ClaudeIntegration,
  type ClaudeIntegrationErrorKind,
  type CommandErrorKind,
  type ClaudeIntegrationPreview,
  describeError,
  isCommandError,
} from "../lib/ipc";
import type { FocusSource } from "../lib/windowFocus";
import { type ClaudeBackend, IntegrationUnavailableError } from "./backend";

/**
 * The Claude Code integration as the window knows it: whether Claude Code
 * is there, whether the plugin of termx is installed, what the user said
 * about it, and the install or removal under way.
 */

export type IntegrationState =
  | { status: "loading" }
  /** This build has no integration: its commands do not exist. */
  | { status: "unavailable" }
  /** Reading it failed; termx tries again (`RETRY_MS`), and on focus. */
  | { status: "failed" }
  | { status: "ready"; info: ClaudeIntegration };

export type Operation =
  | { kind: "idle" }
  | { kind: "installing" }
  | { kind: "removing" }
  | { kind: "failed"; during: "install" | "remove"; error: IntegrationFailure };

/**
 * Why the integration failed: what `getClaudeIntegration` reports
 * (`ClaudeIntegrationError`), or what a command of it rejected with.
 */
export interface IntegrationFailure {
  kind: ClaudeIntegrationErrorKind | CommandErrorKind;
  message: string;
}

export type PreviewState =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ready"; preview: ClaudeIntegrationPreview }
  | { status: "failed"; message: string };

export interface IntegrationSnapshot {
  state: IntegrationState;
  operation: Operation;
  preview: PreviewState;
}

/** What the setup dialog offers, from where the integration stands. */
export type SetupMode = "install" | "update" | "repair";

/** How long to wait before reading the integration again after a failure. */
export const RETRY_MS = [1_000, 2_000, 5_000, 15_000, 60_000] as const;

export class ClaudeIntegrationStore {
  private snapshot: IntegrationSnapshot = {
    state: { status: "loading" },
    operation: { kind: "idle" },
    preview: { status: "idle" },
  };
  private readonly listeners = new Set<() => void>();
  private failures = 0;
  private retry: ReturnType<typeof setTimeout> | null = null;
  private stops: (() => void)[] = [];

  constructor(readonly backend: ClaudeBackend) {}

  /**
   * Reads the integration, and again whenever it may have changed: when the
   * window gets the focus back (Claude Code installed meanwhile, the plugin
   * removed by hand) and when the Rust side says so
   * (`claude-integration-changed`). Returns how to stop.
   */
  start(focus: FocusSource): () => void {
    if (this.stops.length > 0) return this.stop;
    let wasFocused = focus.focused();
    const stopFocus = focus.subscribe(() => {
      const focused = focus.focused();
      if (focused && !wasFocused) void this.load();
      wasFocused = focused;
    });
    let stopChanges: (() => void) | null = null;
    let stopped = false;
    this.backend
      .onClaudeIntegrationChanged(() => void this.load())
      .then(
        (unlisten) => {
          if (stopped) unlisten();
          else stopChanges = unlisten;
        },
        () => undefined,
      );
    this.stops = [
      stopFocus,
      () => {
        stopped = true;
        stopChanges?.();
      },
    ];
    void this.load();
    return this.stop;
  }

  stop = (): void => {
    for (const stop of this.stops) stop();
    this.stops = [];
    if (this.retry !== null) clearTimeout(this.retry);
    this.retry = null;
  };

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getSnapshot = (): IntegrationSnapshot => this.snapshot;

  private set(change: Partial<IntegrationSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...change };
    for (const listener of this.listeners) listener();
  }

  /** What the Rust side says, once known. */
  get info(): ClaudeIntegration | null {
    const state = this.snapshot.state;
    return state.status === "ready" ? state.info : null;
  }

  /** The commands of the integration exist: notifications and the badge can be sent. */
  get available(): boolean {
    return this.snapshot.state.status === "ready";
  }

  /**
   * The plugin reports: what `Session.agent` says can be trusted. Agent
   * states are never shown otherwise, as if they worked.
   */
  get reporting(): boolean {
    const plugin = this.info?.plugin;
    return plugin === "installed" || plugin === "outdated";
  }

  /** The first launch with Claude Code: ask whether to install the plugin. */
  get shouldAsk(): boolean {
    const info = this.info;
    return (
      info !== null &&
      info.claudeInstalled &&
      info.consent === "unasked" &&
      info.plugin === "notInstalled"
    );
  }

  /**
   * Why termx will not install now, or `null`: another `termx` entry in
   * the settings of Claude Code (`conflict`), or hooks turned off by policy
   * (`blocked`). Install is never offered then.
   */
  get refusal(): "conflict" | "blocked" | null {
    const plugin = this.info?.plugin;
    return plugin === "conflict" || plugin === "blocked" ? plugin : null;
  }

  /** What the setup dialog offers now. */
  get setupMode(): SetupMode {
    switch (this.info?.plugin) {
      case "outdated":
        return "update";
      case "error":
        return "repair";
      default:
        return "install";
    }
  }

  /**
   * Reads where the integration stands. Never rejects. After a failure it
   * tries again later, waiting longer each time (`RETRY_MS`); a build
   * without the integration is "unavailable", quietly, and is not asked
   * again.
   */
  load = async (): Promise<void> => {
    if (this.snapshot.state.status === "unavailable") return;
    if (this.retry !== null) clearTimeout(this.retry);
    this.retry = null;
    try {
      const info = await this.backend.getClaudeIntegration();
      this.failures = 0;
      this.set({ state: { status: "ready", info } });
    } catch (error) {
      if (error instanceof IntegrationUnavailableError) {
        this.set({ state: { status: "unavailable" } });
        return;
      }
      // Logged by `call` or by the Rust side. What was known stays.
      if (this.snapshot.state.status !== "ready") this.set({ state: { status: "failed" } });
      const delay = RETRY_MS[Math.min(this.failures, RETRY_MS.length - 1)];
      this.failures += 1;
      this.retry = setTimeout(() => {
        this.retry = null;
        void this.load();
      }, delay);
    }
  };

  /** Reads what installing would change, for the setup dialog. */
  loadPreview = async (): Promise<void> => {
    this.set({ preview: { status: "loading" } });
    try {
      const preview = await this.backend.previewClaudeIntegration();
      this.set({ preview: { status: "ready", preview } });
    } catch (error) {
      this.set({ preview: { status: "failed", message: describeError(error) } });
    }
  };

  /**
   * The user said yes: records it, then installs (or updates) the plugin.
   * Resolves with whether it worked; the error is in `operation`.
   */
  install = async (): Promise<boolean> => {
    if (this.busy) return false;
    this.set({ operation: { kind: "installing" } });
    try {
      if (this.info?.consent !== "accepted") await this.backend.setClaudeConsent("accepted");
      await this.backend.installClaudeIntegration();
      await this.load();
      this.set({ operation: { kind: "idle" } });
      return true;
    } catch (error) {
      this.set({ operation: { kind: "failed", during: "install", error: asError(error) } });
      await this.load();
      return false;
    }
  };

  /** The user said not now: termx does not ask again on its own. */
  decline = async (): Promise<void> => {
    if (this.info?.consent === "declined") return;
    try {
      await this.backend.setClaudeConsent("declined");
    } catch (error) {
      if (!isCommandError(error)) reportError("claude-consent", error);
    }
    await this.load();
  };

  /** Removes the plugin. Resolves with whether it worked. */
  uninstall = async (): Promise<boolean> => {
    if (this.busy) return false;
    this.set({ operation: { kind: "removing" } });
    try {
      await this.backend.uninstallClaudeIntegration();
      // Removed on purpose: not asked again at the next start.
      await this.backend.setClaudeConsent("declined");
      await this.load();
      this.set({ operation: { kind: "idle" } });
      return true;
    } catch (error) {
      this.set({ operation: { kind: "failed", during: "remove", error: asError(error) } });
      await this.load();
      return false;
    }
  };

  /** Forgets the error of the last install or removal. */
  clearError = (): void => {
    if (this.snapshot.operation.kind === "failed") this.set({ operation: { kind: "idle" } });
  };

  private get busy(): boolean {
    const kind = this.snapshot.operation.kind;
    return kind === "installing" || kind === "removing";
  }
}

function asError(error: unknown): IntegrationFailure {
  if (isCommandError(error)) return { kind: error.kind, message: error.message };
  reportError("claude-integration", error);
  return { kind: "internal", message: describeError(error) };
}

/**
 * The words for each kind of failure the integration reports or its
 * commands reject with.
 */
const ERROR_LABELS: Partial<Record<IntegrationFailure["kind"], string>> = {
  invalidSettings: "Your Claude Code settings file is not valid JSON, so termx left it alone",
  conflict:
    "Another “termx” plugin entry exists in your Claude Code settings; termx won’t touch it",
  blocked: "Your organisation’s managed settings disable hooks",
  senderMissing: "This build of termx lacks the program the hooks run",
  permissionDenied:
    "termx is not allowed to write your Claude Code settings (the file is read-only)",
  storageFull: "The disk is full",
  notFound: "Your Claude Code settings file is a link to a file that does not exist",
  io: "termx could not read or write your Claude Code settings",
  internal: "Something went wrong",
};

/** What went wrong, in words, by its kind; a kind not known here shows its message. */
export function describeIntegrationError(error: IntegrationFailure): string {
  const label = Object.hasOwn(ERROR_LABELS, error.kind) ? ERROR_LABELS[error.kind] : undefined;
  if (label === undefined) return error.message;
  if (error.message === "" || error.message === label) return `${label}.`;
  return `${label}: ${error.message}`;
}
