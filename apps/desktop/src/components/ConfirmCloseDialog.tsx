import type { SessionId } from "../lib/ipc";
import { useWorkbench, useWorkspace } from "../workspace/context";
import { KIND_LABELS } from "../workspace/describe";
import { Modal } from "./Modal";

/** Asks before closing a session whose process runs. */
export function ConfirmCloseDialog({ session: id }: { session: SessionId }) {
  const workbench = useWorkbench();
  const workspace = useWorkspace();
  const session = workspace?.sessions.find((each) => each.id === id);
  if (!session) return null;
  return (
    <Modal
      title={`Close “${session.title}”?`}
      role="alertdialog"
      testId="confirm-close-dialog"
      onClose={workbench.closeDialog}
      describedBy="confirm-close-text"
    >
      <p className="modal-text" id="confirm-close-text">
        {KIND_LABELS[session.kind]} is still running in it. Closing the session stops it and removes
        the session from the sidebar.
      </p>
      <div className="modal-actions">
        <button type="button" className="button" onClick={workbench.closeDialog}>
          Cancel
        </button>
        <button
          type="button"
          className="button button-danger"
          data-autofocus
          data-testid="confirm-close"
          onClick={() => void workbench.close(id)}
        >
          Close session
        </button>
      </div>
    </Modal>
  );
}

/** Asks before restarting a session whose process runs. */
export function ConfirmRestartDialog({ session: id }: { session: SessionId }) {
  const workbench = useWorkbench();
  const workspace = useWorkspace();
  const session = workspace?.sessions.find((each) => each.id === id);
  if (!session) return null;
  return (
    <Modal
      title={`Restart “${session.title}”?`}
      role="alertdialog"
      testId="confirm-restart-dialog"
      onClose={workbench.closeDialog}
      describedBy="confirm-restart-text"
    >
      <p className="modal-text" id="confirm-restart-text">
        {KIND_LABELS[session.kind]} is running in it. Restarting stops it, and anything it is in the
        middle of, and starts it again in the same folder.
      </p>
      <div className="modal-actions">
        <button type="button" className="button" data-autofocus onClick={workbench.closeDialog}>
          Cancel
        </button>
        <button
          type="button"
          className="button button-danger"
          data-testid="confirm-restart"
          onClick={() => {
            workbench.confirmRestart(id);
          }}
        >
          Restart
        </button>
      </div>
    </Modal>
  );
}

/** Asks before quitting while sessions run. */
export function ConfirmQuitDialog({ running }: { running: number }) {
  const workbench = useWorkbench();
  return (
    <Modal
      title="Quit termx?"
      role="alertdialog"
      testId="confirm-quit-dialog"
      onClose={workbench.closeDialog}
      describedBy="confirm-quit-text"
    >
      <p className="modal-text" id="confirm-quit-text">
        {running === 1 ? "1 session is running" : `${String(running)} sessions are running`}.
        Quitting stops {running === 1 ? "it" : "them"}; {running === 1 ? "it comes" : "they come"}{" "}
        back, not running, the next time you open termx, ready to be resumed.
      </p>
      <div className="modal-actions">
        <button type="button" className="button" onClick={workbench.closeDialog}>
          Cancel
        </button>
        <button
          type="button"
          className="button button-danger"
          data-autofocus
          data-testid="confirm-quit"
          onClick={() => void workbench.quit()}
        >
          Quit
        </button>
      </div>
    </Modal>
  );
}
