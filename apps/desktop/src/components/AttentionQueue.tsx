import { useMemo } from "react";

import { agentTooltip, attentionQueue, viewAgent } from "../attention/agent";
import type { WorkspaceSnapshot } from "../lib/ipc";
import { useNow } from "../workspace/clock";
import { useAgentsReported, useShortcut, useWorkbench, withShortcut } from "../workspace/context";
import { relativeTime } from "../workspace/describe";
import { sessionOfPane } from "../workspace/layoutTree";
import { StatusIcon } from "../icons/StatusIcon";

/**
 * "Needs you (N)": the sessions whose agent waits for the user, at the top
 * of the sidebar, the one waiting longest first. Each is a button that
 * shows it. Gone while nothing waits.
 */
export function AttentionQueue({ workspace }: { workspace: WorkspaceSnapshot }) {
  const workbench = useWorkbench();
  const reported = useAgentsReported();
  const now = useNow();
  const jump = useShortcut("jumpToWaitingSession");
  const queue = useMemo(() => (reported ? attentionQueue(workspace) : []), [reported, workspace]);
  if (queue.length === 0) return null;
  const active = sessionOfPane(workspace.layout, workspace.layout.focused);

  return (
    <section
      className="attention-queue"
      aria-labelledby="attention-queue-title"
      data-testid="attention-queue"
    >
      <h3
        className="attention-queue-title"
        id="attention-queue-title"
        title={withShortcut("Go to the session waiting longest", jump)}
      >
        Needs you <span className="attention-queue-count">({queue.length})</span>
      </h3>
      <ul className="attention-queue-list">
        {queue.map((session) => {
          const view = viewAgent(session.agent);
          if (!view) return null;
          const since = view.waitingSince ?? view.since;
          const tooltip = agentTooltip(view, now);
          return (
            <li key={session.id}>
              <button
                type="button"
                className="attention-item"
                data-testid="attention-item"
                data-session-id={session.id}
                data-active={session.id === active || undefined}
                aria-label={`${session.title}, ${session.project.name}: ${tooltip}`}
                title={`${session.title} · ${tooltip}`}
                onClick={() => void workbench.activate(session.id)}
              >
                <StatusIcon status={view.icon} size={12} decorative />
                <span className="attention-item-title" dir="auto">
                  {session.title}
                </span>
                <span className="attention-item-project" dir="auto">
                  {session.project.name}
                </span>
                {since !== null && (
                  <span className="attention-item-time">{relativeTime(now, since)}</span>
                )}
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
