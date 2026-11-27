import {
  type DragEvent,
  type KeyboardEvent,
  memo,
  type MouseEvent,
  useEffect,
  useRef,
} from "react";

import type { Session, SessionId } from "../lib/ipc";
import { useNow } from "../workspace/clock";
import {
  describeAgent,
  describeState,
  KIND_LABELS,
  relativeTime,
  relativeTimeLong,
} from "../workspace/describe";
import { AgentSlot } from "./AgentSlot";
import { rowElementId } from "./elementIds";
import { BranchIcon, KindIcon } from "./icons";

export interface SessionRowProps {
  session: Session;
  /** Shown in the focused pane. */
  active: boolean;
  /** Shown in some pane. */
  visible: boolean;
  /** The item of the sidebar that takes the Tab key. */
  tabbable: boolean;
  renaming: boolean;
  /** Where a session being dragged over this row would land. */
  dropAt: "before" | "after" | null;
  /** Shows the project name: in the background section, rows mix projects. */
  showProject?: boolean;
  /** What the row does, the same object for every row and every render. */
  actions: RowActions;
}

/**
 * What a row can do, by the id of its session. The tree passes one object
 * that never changes, so that a row only renders again when its session or
 * its own flags change.
 */
export interface RowActions {
  activate: (id: SessionId) => void;
  rename: (id: SessionId, name: string) => void;
  cancelRename: () => void;
  startRename: (id: SessionId) => void;
  resume: (id: SessionId) => void;
  contextMenu: (id: SessionId, event: MouseEvent) => void;
  focus: (id: SessionId) => void;
  dragStart: (id: SessionId, event: DragEvent) => void;
  dragOver: (id: SessionId, event: DragEvent) => void;
  drop: (id: SessionId, event: DragEvent) => void;
  dragEnd: () => void;
}

/**
 * One session in the sidebar.
 *
 * ```text
 * [kind] Title, on up to two lines                [agent]
 *        branch  worktree        status       last activity
 * ```
 */
export const SessionRow = memo(function SessionRow(props: SessionRowProps) {
  const { session, actions } = props;
  const now = useNow();
  const status = describeState(session);
  const agent = describeAgent(session.agent);
  const branch = session.git ? (session.git.branch ?? session.git.commit) : null;
  const dormant = session.state.status === "dormant";
  const attention = agent?.tone === "attention";

  const label = [
    session.title,
    KIND_LABELS[session.kind],
    status.label,
    branch ? `branch ${branch}` : null,
    session.git?.worktree ? `worktree ${session.git.worktree}` : null,
    props.showProject ? `project ${session.project.name}` : null,
    agent ? `agent: ${agent.label}` : null,
    `active ${relativeTimeLong(now, session.lastActivityAt)}`,
  ]
    .filter((part) => part !== null)
    .join(", ");

  return (
    <li
      id={rowElementId(session.id)}
      role="treeitem"
      aria-level={2}
      aria-selected={props.active}
      aria-label={label}
      tabIndex={props.tabbable ? 0 : -1}
      className="session-row"
      data-testid="session-row"
      data-session-id={session.id}
      data-status={session.state.status}
      data-active={props.active || undefined}
      data-visible={props.visible || undefined}
      data-attention={attention || undefined}
      data-drop={props.dropAt ?? undefined}
      draggable={!props.renaming}
      onClick={(event) => {
        if (props.renaming) return;
        event.stopPropagation();
        actions.activate(session.id);
      }}
      onDoubleClick={(event) => {
        event.stopPropagation();
        actions.startRename(session.id);
      }}
      onContextMenu={(event) => {
        actions.contextMenu(session.id, event);
      }}
      onFocus={(event) => {
        if (event.target === event.currentTarget) actions.focus(session.id);
      }}
      onDragStart={(event) => {
        actions.dragStart(session.id, event);
      }}
      onDragOver={(event) => {
        actions.dragOver(session.id, event);
      }}
      onDrop={(event) => {
        actions.drop(session.id, event);
      }}
      onDragEnd={actions.dragEnd}
    >
      <div className="row-line">
        <KindIcon kind={session.kind} />
        {props.renaming ? (
          <RenameInput
            session={session}
            onCommit={(name) => {
              actions.rename(session.id, name);
            }}
            onCancel={actions.cancelRename}
          />
        ) : (
          <span className="row-title" title={session.title}>
            {session.title}
          </span>
        )}
        <AgentSlot agent={session.agent} />
      </div>
      <div className="row-line row-meta">
        {props.showProject && <span className="row-project">{session.project.name}</span>}
        {branch !== null && (
          <span className="row-branch" title={`Branch ${branch}`}>
            <BranchIcon />
            <span className="row-branch-name">{branch}</span>
          </span>
        )}
        {session.git?.worktree && (
          <span
            className="row-worktree"
            data-testid="worktree-marker"
            title={`Worktree ${session.git.worktree}`}
          >
            wt:{session.git.worktree}
          </span>
        )}
        <span
          className="row-status"
          data-tone={status.tone}
          data-testid="session-status"
          title={status.detail ?? status.label}
        >
          <span className="status-dot" aria-hidden="true" />
          {status.label}
        </span>
        {dormant && (
          <button
            type="button"
            className="row-resume"
            tabIndex={-1}
            data-testid="row-resume"
            onClick={(event) => {
              event.stopPropagation();
              actions.resume(session.id);
            }}
          >
            Resume
          </button>
        )}
        <time
          className="row-time"
          dateTime={new Date(session.lastActivityAt).toISOString()}
          title={`Last activity ${new Date(session.lastActivityAt).toLocaleString()}`}
        >
          {relativeTime(now, session.lastActivityAt)}
        </time>
      </div>
    </li>
  );
});

function RenameInput({
  session,
  onCommit,
  onCancel,
}: {
  session: Session;
  onCommit: (name: string) => void;
  onCancel: () => void;
}) {
  const input = useRef<HTMLInputElement>(null);
  const done = useRef(false);
  const initial = session.name ?? session.title;

  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, []);

  const commit = () => {
    if (done.current) return;
    done.current = true;
    const value = input.current?.value ?? initial;
    if (value.trim() === initial.trim()) onCancel();
    else onCommit(value);
  };

  return (
    <input
      ref={input}
      className="rename-input"
      data-testid="rename-input"
      aria-label="Session name. Leave empty to use the automatic title."
      defaultValue={initial}
      placeholder={session.autoTitle ?? KIND_LABELS[session.kind]}
      onClick={(event) => {
        event.stopPropagation();
      }}
      onKeyDown={(event: KeyboardEvent) => {
        // The keys of the input stay in it, away from the tree.
        event.stopPropagation();
        if (event.key === "Enter") {
          event.preventDefault();
          commit();
        } else if (event.key === "Escape") {
          event.preventDefault();
          done.current = true;
          onCancel();
        }
      }}
      onBlur={commit}
    />
  );
}
