import { describeIntegrationError } from "../claude/integration";
import {
  useClaudeIntegration,
  useNotificationSettings,
  useUi,
  useWorkbench,
} from "../workspace/context";
import { InfoIcon } from "../icons/glyphs";
import { Modal } from "./Modal";

/**
 * The Claude Code integration, from the user's side: the question asked
 * once (a banner that opens the setup dialog), the setup dialog with the
 * exact list of what changes, and a line in the sidebar footer that says
 * where it stands and offers to enable, update or remove it.
 */

/** Why termx installs nothing, by the status of the plugin. */
const REFUSALS = {
  conflict:
    "Another “termx” plugin entry exists in your Claude Code settings; termx won’t touch it.",
  blocked:
    "Your organisation’s managed settings disable hooks, so Claude Code does not run those of termx.",
} as const;

const WHAT_IT_DOES =
  "termx shows when each Claude session is working or needs you: a mark on its row, a list of the sessions waiting for you, and a notification when you are looking elsewhere.";

/**
 * Asks, once, whether to install the plugin: on the first launch where
 * Claude Code is found and the user was never asked. Not modal.
 */
export function ClaudeConsentBanner() {
  const workbench = useWorkbench();
  const ui = useUi();
  // Renders again when the integration changes.
  useClaudeIntegration();
  if (!workbench.claude.shouldAsk || ui.dialog.kind === "claudeSetup") return null;
  return (
    <div className="banner" data-tone="info" data-testid="claude-consent-banner" role="status">
      <InfoIcon className="banner-icon" />
      <span className="banner-text" title={WHAT_IT_DOES}>
        See which Claude session needs you: termx can show when each one is working or waiting.
      </span>
      <button
        type="button"
        className="button button-primary button-small"
        data-testid="claude-consent-open"
        onClick={workbench.openClaudeSetup}
      >
        Set up…
      </button>
      <button
        type="button"
        className="button button-small"
        data-testid="claude-consent-not-now"
        onClick={() => void workbench.declineClaude()}
      >
        Not now
      </button>
    </div>
  );
}

/** What the plugin does and changes, with Install and Not now. */
export function ClaudeSetupDialog() {
  const workbench = useWorkbench();
  const { preview, operation } = useClaudeIntegration();
  const mode = workbench.claude.setupMode;
  const installing = operation.kind === "installing";
  const failed = operation.kind === "failed" && operation.during === "install" ? operation : null;
  const title =
    mode === "update"
      ? "Update the Claude Code integration"
      : mode === "repair"
        ? "Repair the Claude Code integration"
        : "Show Claude Code status in termx";
  const action = mode === "update" ? "Update" : mode === "repair" ? "Repair" : "Install";
  const progress =
    mode === "update" ? "Updating…" : mode === "repair" ? "Repairing…" : "Installing…";
  // Nothing is installed blind: only once the user saw what changes.
  const refusal = workbench.claude.refusal;
  const canInstall = preview.status === "ready" && !installing && refusal === null;

  return (
    <Modal
      title={title}
      testId="claude-setup-dialog"
      onClose={() => {
        if (!installing) workbench.closeDialog();
      }}
      describedBy="claude-setup-text"
    >
      <p className="modal-text" id="claude-setup-text">
        {mode === "update"
          ? "A newer version of the plugin of termx for Claude Code is available. Updating it changes:"
          : `${WHAT_IT_DOES} To do that, termx adds a small plugin to Claude Code, which changes:`}
      </p>
      {refusal !== null && (
        <p className="form-error" role="alert" data-testid="claude-refusal">
          termx cannot install the plugin. {REFUSALS[refusal]}
        </p>
      )}
      {preview.status === "loading" && (
        <p className="modal-text" role="status">
          Checking what would change…
        </p>
      )}
      {preview.status === "failed" && (
        <p className="form-error" role="alert">
          termx could not tell what would change: {preview.message}{" "}
          <button
            type="button"
            className="link-button"
            onClick={() => void workbench.claude.loadPreview()}
          >
            Retry
          </button>
        </p>
      )}
      {preview.status === "ready" && (
        <ul className="claude-changes" data-testid="claude-changes">
          {preview.preview.changes.map((change) => (
            <li key={change.path} className="claude-change">
              <code className="claude-change-path">{change.path}</code>
              <span className="claude-change-description">{change.description}</span>
            </li>
          ))}
        </ul>
      )}
      {preview.status === "ready" && (
        <p className="modal-text claude-note" data-testid="claude-note">
          {preview.preview.note ??
            "Existing hooks are kept; termx only adds the entries listed above."}{" "}
          You can remove them at any time, from the bottom of the sidebar.
        </p>
      )}
      {installing && (
        <p className="modal-text" role="status" data-testid="claude-installing">
          {progress}
        </p>
      )}
      {failed && (
        <p className="form-error" role="alert" data-testid="claude-install-error">
          {action} failed. {describeIntegrationError(failed.error)}
        </p>
      )}
      {mode === "install" && (
        <p className="modal-text claude-later" id="claude-later">
          Not now? You can enable it later from the sidebar.
        </p>
      )}
      <div className="modal-actions">
        <button
          type="button"
          className="button"
          data-testid="claude-not-now"
          data-autofocus
          aria-describedby={mode === "install" ? "claude-later" : undefined}
          disabled={installing}
          onClick={() => {
            if (mode === "install") void workbench.declineClaude();
            else workbench.closeDialog();
          }}
        >
          Not now
        </button>
        {refusal === null && (
          <button
            type="button"
            className="button button-primary"
            data-testid="claude-install"
            disabled={!canInstall}
            onClick={() => void workbench.installClaude()}
          >
            {installing ? progress : failed ? "Try again" : action}
          </button>
        )}
      </div>
    </Modal>
  );
}

/** Asks before removing the plugin. */
export function ClaudeRemoveDialog() {
  const workbench = useWorkbench();
  const { operation } = useClaudeIntegration();
  const removing = operation.kind === "removing";
  const failed = operation.kind === "failed" && operation.during === "remove" ? operation : null;
  return (
    <Modal
      title="Remove the Claude Code integration?"
      role="alertdialog"
      testId="claude-remove-dialog"
      onClose={() => {
        if (!removing) workbench.closeDialog();
      }}
      describedBy="claude-remove-text"
    >
      <p className="modal-text" id="claude-remove-text">
        termx removes its plugin from Claude Code. Sessions will no longer show whether they are
        working or need you. Your own hooks and settings are not touched.
      </p>
      {failed && (
        <p className="form-error" role="alert" data-testid="claude-remove-error">
          Removing failed. {describeIntegrationError(failed.error)}
        </p>
      )}
      <div className="modal-actions">
        <button
          type="button"
          className="button"
          data-autofocus
          disabled={removing}
          onClick={workbench.closeDialog}
        >
          Cancel
        </button>
        <button
          type="button"
          className="button button-danger"
          data-testid="claude-remove-confirm"
          disabled={removing}
          onClick={() => void workbench.removeClaude()}
        >
          {removing ? "Removing…" : "Remove"}
        </button>
      </div>
    </Modal>
  );
}

/**
 * The line of the sidebar footer: where the integration stands, and what
 * can be done about it. Nothing while termx cannot tell.
 */
export function ClaudeStatus() {
  const workbench = useWorkbench();
  const { state } = useClaudeIntegration();
  if (state.status !== "ready") return null;
  const info = state.info;

  if (!info.claudeInstalled && info.plugin === "notInstalled") {
    return (
      <div className="claude-status" data-testid="claude-status" data-state="claude-missing">
        <p className="claude-status-text">
          Claude Code status is off: Claude Code was not found on this computer. Install it to see
          when each session needs you.
        </p>
        <button type="button" className="link-button" onClick={() => void workbench.claude.load()}>
          Check again
        </button>
      </div>
    );
  }

  switch (info.plugin) {
    case "installed":
      return (
        <div className="claude-status" data-testid="claude-status" data-state="installed">
          <p className="claude-status-text">
            Claude Code integration: installed{info.version ? ` v${info.version}` : ""} ·{" "}
            <button
              type="button"
              className="link-button"
              data-testid="claude-remove"
              onClick={workbench.openClaudeRemove}
            >
              Remove
            </button>
          </p>
          <NotificationToggles />
        </div>
      );
    case "outdated":
      return (
        <div className="claude-status" data-testid="claude-status" data-state="outdated">
          <p className="claude-status-text">
            Claude Code integration: an update is available
            {info.version ? ` (installed v${info.version})` : ""} ·{" "}
            <button
              type="button"
              className="link-button"
              data-testid="claude-update"
              onClick={workbench.openClaudeSetup}
            >
              Update…
            </button>{" "}
            ·{" "}
            <button
              type="button"
              className="link-button"
              data-testid="claude-remove"
              onClick={workbench.openClaudeRemove}
            >
              Remove
            </button>
          </p>
          <NotificationToggles />
        </div>
      );
    case "error":
      return (
        <div className="claude-status" data-testid="claude-status" data-state="error">
          <p className="claude-status-text" data-tone="error">
            Claude Code integration has a problem
            {info.error ? `: ${describeIntegrationError(info.error)}` : "."}
          </p>
          <p className="claude-status-text">
            <button
              type="button"
              className="link-button"
              data-testid="claude-repair"
              onClick={workbench.openClaudeSetup}
            >
              Repair…
            </button>{" "}
            ·{" "}
            <button
              type="button"
              className="link-button"
              data-testid="claude-remove"
              onClick={workbench.openClaudeRemove}
            >
              Remove
            </button>
          </p>
        </div>
      );
    case "conflict":
      return (
        <div className="claude-status" data-testid="claude-status" data-state="conflict">
          <p className="claude-status-text" data-tone="warn">
            Claude Code status is off. {REFUSALS.conflict}
          </p>
          <button
            type="button"
            className="link-button"
            onClick={() => void workbench.claude.load()}
          >
            Check again
          </button>
        </div>
      );
    case "blocked":
      return (
        <div className="claude-status" data-testid="claude-status" data-state="blocked">
          <p className="claude-status-text" data-tone="warn">
            Claude Code integration: installed, but not running. {REFUSALS.blocked}
          </p>
          <p className="claude-status-text">
            <button
              type="button"
              className="link-button"
              onClick={() => void workbench.claude.load()}
            >
              Check again
            </button>{" "}
            ·{" "}
            <button
              type="button"
              className="link-button"
              data-testid="claude-remove"
              onClick={workbench.openClaudeRemove}
            >
              Remove
            </button>
          </p>
        </div>
      );
    default:
      return (
        <div className="claude-status" data-testid="claude-status" data-state="not-installed">
          <button
            type="button"
            className="link-button"
            data-testid="claude-enable"
            title={WHAT_IT_DOES}
            onClick={workbench.openClaudeSetup}
          >
            Enable Claude Code status
          </button>
        </div>
      );
  }
}

/** When to send OS notifications. */
function NotificationToggles() {
  const workbench = useWorkbench();
  const settings = useNotificationSettings();
  return (
    <div className="notification-toggles">
      <label className="toggle">
        <input
          type="checkbox"
          data-testid="notify-waiting"
          checked={settings.waiting}
          onChange={(event) => {
            workbench.notificationPrefs.set({ waiting: event.target.checked });
          }}
        />
        Notify me when a session needs me
      </label>
      <label className="toggle">
        <input
          type="checkbox"
          data-testid="notify-done"
          checked={settings.done}
          onChange={(event) => {
            workbench.notificationPrefs.set({ done: event.target.checked });
          }}
        />
        Also when a session is done
      </label>
    </div>
  );
}
