import * as ipc from "../lib/ipc";

/**
 * The commands of the Claude Code integration the interface uses, in one
 * place: the window goes through this object, never through `ipc.ts`
 * directly, so that tests (and, until the Rust side has them, a build
 * without them) swap every command at once.
 */
export interface ClaudeBackend {
  getClaudeIntegration: typeof ipc.getClaudeIntegration;
  previewClaudeIntegration: typeof ipc.previewClaudeIntegration;
  installClaudeIntegration: typeof ipc.installClaudeIntegration;
  uninstallClaudeIntegration: typeof ipc.uninstallClaudeIntegration;
  setClaudeConsent: typeof ipc.setClaudeConsent;
  notify: typeof ipc.notify;
  setAttentionBadge: typeof ipc.setAttentionBadge;
  onNotificationClicked: typeof ipc.onNotificationClicked;
  onClaudeIntegrationChanged: typeof ipc.onClaudeIntegrationChanged;
}

/**
 * No integration: every command rejects, so the window shows nothing about
 * agents and sends no notification. The default where no backend is given.
 */
export const NO_CLAUDE: ClaudeBackend = {
  getClaudeIntegration: unavailable,
  previewClaudeIntegration: unavailable,
  installClaudeIntegration: unavailable,
  uninstallClaudeIntegration: unavailable,
  setClaudeConsent: unavailable,
  notify: unavailable,
  setAttentionBadge: unavailable,
  onNotificationClicked: () => Promise.resolve(() => undefined),
  onClaudeIntegrationChanged: () => Promise.resolve(() => undefined),
};

/** What `NO_CLAUDE` rejects with: there is no integration to ask. */
export class IntegrationUnavailableError extends Error {
  constructor() {
    super("this window has no Claude Code integration");
    this.name = "IntegrationUnavailableError";
  }
}

function unavailable(): Promise<never> {
  return Promise.reject(new IntegrationUnavailableError());
}

/** The real commands, on the Rust side. */
export const TAURI_CLAUDE: ClaudeBackend = {
  getClaudeIntegration: ipc.getClaudeIntegration,
  previewClaudeIntegration: ipc.previewClaudeIntegration,
  installClaudeIntegration: ipc.installClaudeIntegration,
  uninstallClaudeIntegration: ipc.uninstallClaudeIntegration,
  setClaudeConsent: ipc.setClaudeConsent,
  notify: ipc.notify,
  setAttentionBadge: ipc.setAttentionBadge,
  onNotificationClicked: ipc.onNotificationClicked,
  onClaudeIntegrationChanged: ipc.onClaudeIntegrationChanged,
};
