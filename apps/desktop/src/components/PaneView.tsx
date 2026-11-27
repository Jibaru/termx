import { type DragEvent, useEffect, useState } from "react";

import { isWaiting } from "../attention/agent";
import type { PaneEdge, Session, SessionId, WorkspaceSnapshot } from "../lib/ipc";
import {
  useAgentsReported,
  useShortcut,
  useUi,
  useWorkbench,
  withShortcut,
} from "../workspace/context";
import { describeState, failureLabel, statusOf } from "../workspace/describe";
import { buildSidebar } from "../workspace/grouping";
import { visibleSessions } from "../workspace/layoutTree";
import { BranchIcon, CloseIcon, ResumeIcon, SplitDownIcon, SplitRightIcon } from "../icons/glyphs";
import { KindIcon } from "../icons/KindIcon";
import { StatusIcon } from "../icons/StatusIcon";
import { AgentSlot } from "./AgentSlot";
import { TerminalView } from "./TerminalView";

interface PaneViewProps {
  pane: number;
  sessionId: SessionId | null;
  workspace: WorkspaceSnapshot;
  focused: boolean;
  /** The only pane of the window. */
  alone: boolean;
}

/**
 * One pane of the main area: a header with the session it shows and the
 * pane's own actions, and the terminal of that session, or an empty state.
 * A click anywhere in the pane gives it the focus.
 */
export function PaneView({ pane, sessionId, workspace, focused, alone }: PaneViewProps) {
  const workbench = useWorkbench();
  const ui = useUi();
  const session =
    sessionId === null ? undefined : workspace.sessions.find((each) => each.id === sessionId);
  const splitRight = useShortcut("splitRight");
  const splitDown = useShortcut("splitDown");
  const closePane = useShortcut("closePane");
  const controller = session ? workbench.terminals.controllerFor(session) : undefined;
  const agentsShown = useAgentsReported();
  const agent = agentsShown ? (session?.agent ?? null) : null;
  const attention = agent !== null && session !== undefined && isWaiting(session);

  // The focused pane gives the keyboard to its terminal when it becomes so.
  useEffect(() => {
    if (focused) controller?.focus();
  }, [focused, controller]);

  return (
    <section
      className="pane"
      data-testid="pane"
      data-pane-id={pane}
      data-focused={focused || undefined}
      data-alone={alone || undefined}
      data-attention={attention || undefined}
      aria-label={
        session ? `Pane: ${session.title}${attention ? ", needs you" : ""}` : "Empty pane"
      }
      onMouseDown={() => {
        void workbench.focusPane(pane);
      }}
    >
      <header className="pane-header">
        {session ? (
          <>
            <KindIcon kind={session.kind} />
            <span className="pane-title" dir="auto" title={session.title}>
              {session.title}
            </span>
            {session.git?.branch && (
              <span className="pane-branch">
                <BranchIcon />
                {session.git.branch}
              </span>
            )}
            <span className="row-status" data-tone={describeState(session).tone}>
              <StatusIcon status={statusOf(session)} size={12} decorative />
              {describeState(session).label}
            </span>
            <AgentSlot agent={agent} testId="pane-agent" />
          </>
        ) : (
          <span className="pane-title pane-title-empty">Empty pane</span>
        )}
        <span className="pane-actions">
          <button
            type="button"
            className="icon-button"
            data-testid="pane-split-right"
            aria-label={withShortcut("Split right", splitRight)}
            title={withShortcut("Split right", splitRight)}
            onClick={() => {
              void workbench.focusPane(pane).then(() => workbench.split("row"));
            }}
          >
            <SplitRightIcon />
          </button>
          <button
            type="button"
            className="icon-button"
            data-testid="pane-split-down"
            aria-label={withShortcut("Split down", splitDown)}
            title={withShortcut("Split down", splitDown)}
            onClick={() => {
              void workbench.focusPane(pane).then(() => workbench.split("column"));
            }}
          >
            <SplitDownIcon />
          </button>
          {!alone && (
            <button
              type="button"
              className="icon-button"
              data-testid="pane-close"
              aria-label={withShortcut("Close pane", closePane)}
              title={withShortcut("Close pane (the session keeps running)", closePane)}
              onClick={(event) => {
                event.stopPropagation();
                void workbench.closePane(pane);
              }}
            >
              <CloseIcon />
            </button>
          )}
        </span>
      </header>
      <div className="pane-body">
        {session && controller ? (
          <>
            <TerminalView key={session.id} controller={controller} autoFocus={focused} />
            <PaneNotice session={session} />
          </>
        ) : (
          <EmptyPane pane={pane} workspace={workspace} />
        )}
        {ui.dragging !== null && <DropZones pane={pane} dragged={ui.dragging} />}
      </div>
    </section>
  );
}

/**
 * What the pane says over the terminal while its session does not run, or
 * runs with a warning, with the button that starts it again.
 */
function PaneNotice({ session }: { session: Session }) {
  const workbench = useWorkbench();
  const state = session.state;
  if (state.status === "dormant") {
    return (
      <div className="pane-notice" data-testid="pane-notice" data-tone="neutral">
        <StatusIcon status="dormant" decorative />
        <span>
          Not running.{" "}
          {session.kind === "claude"
            ? "Resume carries on with its conversation."
            : "Resume starts it again in the same folder."}
        </span>
        <button
          type="button"
          className="button button-primary"
          data-testid="pane-resume"
          onClick={() => {
            workbench.resume(session.id);
          }}
        >
          <ResumeIcon size={12} />
          Resume
        </button>
        <span className="pane-notice-hint">or press Enter</span>
      </div>
    );
  }
  if (state.status === "exited") {
    const status = describeState(session);
    const claude = session.kind === "claude";
    return (
      <div className="pane-notice" data-testid="pane-notice" data-tone={status.tone}>
        <StatusIcon status={statusOf(session)} decorative />
        <span title={status.detail}>{status.label}.</span>
        <button
          type="button"
          className="button button-primary"
          data-testid="pane-restart"
          onClick={() => {
            if (claude) workbench.resume(session.id);
            else workbench.restart(session.id);
          }}
        >
          {claude ? "Resume" : "Restart"}
        </button>
      </div>
    );
  }
  if (session.warning) {
    return (
      <div className="pane-notice" data-testid="pane-notice" data-tone="warn" role="status">
        <StatusIcon status="warning" decorative />
        <span title={session.warning.message}>{failureLabel(session.warning)}.</span>
      </div>
    );
  }
  return null;
}

/** An empty pane: what can go in it. */
function EmptyPane({ pane, workspace }: { pane: number; workspace: WorkspaceSnapshot }) {
  const workbench = useWorkbench();
  const newSession = useShortcut("newSession");
  const shown = visibleSessions(workspace.layout);
  const order = buildSidebar(workspace).order;
  const hidden = order
    .filter((id) => !shown.has(id))
    .map((id) => workspace.sessions.find((session) => session.id === id))
    .filter((session): session is Session => session !== undefined)
    .slice(0, 8);

  return (
    <div className="empty-pane" data-testid="empty-pane">
      <p className="empty-pane-title">This pane is empty.</p>
      <button
        type="button"
        className="button button-primary"
        data-testid="empty-pane-new-session"
        onClick={() => {
          void workbench.focusPane(pane);
          workbench.openNewSessionDialog();
        }}
      >
        {withShortcut("New session…", newSession)}
      </button>
      {hidden.length > 0 && (
        <>
          <p className="empty-pane-hint">Or show a session here:</p>
          <ul className="empty-pane-list">
            {hidden.map((session) => (
              <li key={session.id}>
                <button
                  type="button"
                  className="link-button"
                  onClick={() => {
                    void workbench.showInPane(pane, session.id);
                  }}
                >
                  <KindIcon kind={session.kind} /> {session.title}
                </button>
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}

const ZONES: { edge: PaneEdge; label: string }[] = [
  { edge: "center", label: "Show here" },
  { edge: "left", label: "Split left" },
  { edge: "right", label: "Split right" },
  { edge: "top", label: "Split up" },
  { edge: "bottom", label: "Split down" },
];

/**
 * Where a session dragged from the sidebar can be dropped on a pane: in its
 * middle to show it there, on an edge to split the pane and show it in the
 * new half.
 */
function DropZones({ pane, dragged }: { pane: number; dragged: SessionId }) {
  const workbench = useWorkbench();
  const [over, setOver] = useState<PaneEdge | null>(null);

  const drop = (event: DragEvent, edge: PaneEdge) => {
    event.preventDefault();
    workbench.setDragging(null);
    if (edge === "center") {
      void workbench.showInPane(pane, dragged);
      return;
    }
    const direction = edge === "left" || edge === "right" ? "row" : "column";
    const placement = edge === "left" || edge === "top" ? "before" : "after";
    void workbench.splitWith(pane, direction, placement, dragged);
  };

  return (
    <div className="drop-zones" data-testid="drop-zones">
      {ZONES.map(({ edge, label }) => (
        <div
          key={edge}
          className="drop-zone"
          data-edge={edge}
          data-over={over === edge || undefined}
          data-testid={`drop-zone-${edge}`}
          onDragOver={(event) => {
            event.preventDefault();
            event.dataTransfer.dropEffect = "move";
            setOver(edge);
          }}
          onDragLeave={() => {
            setOver((current) => (current === edge ? null : current));
          }}
          onDrop={(event) => {
            drop(event, edge);
          }}
        >
          {over === edge && <span className="drop-zone-label">{label}</span>}
        </div>
      ))}
    </div>
  );
}
