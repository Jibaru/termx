import { useEffect } from "react";

import { useUi, useWorkbench, useWorkspace } from "../workspace/context";
import { persistenceMessage } from "../workspace/describe";
import { CloseIcon, InfoIcon, WarningIcon } from "../icons/glyphs";

/**
 * A strip that stays for as long as the workspace is not saved: saving
 * fails (a full disk), or another termx owns the state. It cannot be
 * dismissed, and it takes one line.
 */
export function PersistenceBanner() {
  const workspace = useWorkspace();
  if (!workspace) return null;
  const message = persistenceMessage(workspace.persistence);
  if (message === null) return null;
  return (
    <div
      className="banner"
      data-tone={workspace.persistence.mode === "failing" ? "error" : "warn"}
      data-testid="persistence-warning"
      role="status"
    >
      <WarningIcon className="banner-icon" />
      <span className="banner-text" title={message}>
        {message}
      </span>
    </div>
  );
}

/**
 * Says that sessions were restored from the last run and do not run, with
 * a button that resumes them all. Not modal: each one can also be resumed
 * on its own, or left alone.
 */
export function RestoreBanner() {
  const workbench = useWorkbench();
  const ui = useUi();
  // Read again whenever the workspace changes.
  useWorkspace();
  const waiting = workbench.waitingRestored();
  if (ui.restoreDismissed || waiting.length === 0) return null;
  const count = waiting.length;
  return (
    <div className="banner" data-tone="info" data-testid="restore-banner" role="status">
      <InfoIcon className="banner-icon" />
      <span className="banner-text">
        {count === 1
          ? "1 session was restored from the last run and is not running."
          : `${String(count)} sessions were restored from the last run and are not running.`}
      </span>
      <button
        type="button"
        className="button button-primary button-small"
        data-testid="resume-all"
        onClick={workbench.resumeAll}
      >
        {count === 1 ? "Resume" : "Resume all"}
      </button>
      <button
        type="button"
        className="icon-button"
        aria-label="Dismiss"
        title="Dismiss"
        onClick={workbench.dismissRestore}
      >
        <CloseIcon />
      </button>
    </div>
  );
}

/** How long a notice stays, unless dismissed earlier. */
const NOTICE_MS = 10_000;

/** What went wrong, in a corner of the window, for a few seconds. */
export function Notices() {
  const workbench = useWorkbench();
  const { notices } = useUi();

  useEffect(() => {
    const oldest = notices[0];
    if (!oldest) return;
    const timer = setTimeout(() => {
      workbench.dismissNotice(oldest.id);
    }, NOTICE_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [notices, workbench]);

  if (notices.length === 0) return null;
  return (
    <div className="notices" data-testid="notices">
      {notices.map((notice) => (
        <div key={notice.id} className="notice" role="alert">
          <WarningIcon className="notice-icon" tone="danger" />
          <span>{notice.text}</span>
          <button
            type="button"
            className="icon-button"
            aria-label="Dismiss"
            onClick={() => {
              workbench.dismissNotice(notice.id);
            }}
          >
            <CloseIcon />
          </button>
        </div>
      ))}
    </div>
  );
}
