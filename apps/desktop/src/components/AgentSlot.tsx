import { agentTooltip, viewAgent } from "../attention/agent";
import type { AgentState } from "../lib/ipc";
import { useNow } from "../workspace/clock";
import { StatusIcon } from "../icons/StatusIcon";
import { relativeTime } from "../workspace/describe";

/**
 * What the agent of a session does: working, waiting for the user, done.
 * Empty while the session reports nothing (`agent` is `null`), and while
 * the plugin of termx is not installed in Claude Code. A status this
 * version does not know shows as unknown, and a stale report dimmed: both
 * neutral, never loud.
 */
export function AgentSlot({
  agent,
  testId = "agent-slot",
}: {
  agent: AgentState | null;
  testId?: string;
}) {
  const now = useNow();
  const view = viewAgent(agent);
  if (!view) {
    return <span className="agent-slot" data-testid={testId} data-status="none" />;
  }
  const since = view.waiting ? (view.waitingSince ?? view.since) : null;
  const tooltip = agentTooltip(view, now);
  return (
    <span
      className="agent-slot"
      data-testid={testId}
      data-status={view.icon}
      data-tone={view.tone}
      data-stale={view.stale || undefined}
      title={tooltip}
      aria-label={tooltip}
      role="img"
    >
      <StatusIcon status={view.icon} size={12} decorative />
      <span aria-hidden="true">
        {view.label}
        {since !== null && ` · ${relativeTime(now, since)}`}
      </span>
    </span>
  );
}
