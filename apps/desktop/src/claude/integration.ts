import { reportError } from "../lib/errorReporting";
import {
  type ClaudeIntegration,
  type ClaudeIntegrationErrorKind,
  type CommandErrorKind,
  type ClaudeIntegrationPreview,
  describeError,
  isCommandError,
} from "../lib/ipc";
import type { ClaudeBackend } from "./backend";

/**
 * The Claude Code integration as the window knows it: whether Claude Code
 * is there, whether the plugin of termx is installed, what the user said
 * about it, and the install or removal under way.
 */

export type IntegrationState =
  | { status: "loading" }
  /** The Rust side does not answer: this build has no integration. */
  | { status: "unavailable" }
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

export class ClaudeIntegrationStore {
  private snapshot: IntegrationSnapshot = {
    state: { status: "loading" },
    operation: { kind: "idle" },
    preview: { status: "idle" },
  };
  private readonly listeners = new Set<() => void>();

  constructor(readonly backend: ClaudeBackend) {}

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
    return info !== null && info.claudeInstalled && info.consent === "unasked";
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

  /** Reads where the integration stands. Never rejects. */
  load = async (): Promise<void> => {
    try {
      const info = await this.backend.getClaudeIntegration();
      this.set({ state: { status: "ready", info } });
    } catch {
      // A build whose Rust side has no integration: nothing to show. The
      // failure was logged by `call` or by the Rust side.
      if (this.snapshot.state.status !== "ready") this.set({ state: { status: "unavailable" } });
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

const ERROR_LABELS: Record<string, string> = {
  claudeNotFound: "Claude Code was not found on this computer",
  storageFull: "The disk is full",
  permissionDenied: "termx is not allowed to write to the folder of Claude Code",
  notFound: "A file termx needs was not found",
  invalidArgument: "Claude Code did not accept the plugin",
  internal: "Something went wrong",
};

/** What went wrong, in words, by its kind; a kind not known here shows its message. */
export function describeIntegrationError(error: IntegrationFailure): string {
  const label = ERROR_LABELS[error.kind];
  if (label === undefined) return error.message;
  if (error.message === "" || error.message === label) return `${label}.`;
  return `${label}: ${error.message}`;
}
