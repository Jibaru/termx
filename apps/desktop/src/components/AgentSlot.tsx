import type { AgentState } from "../lib/ipc";
import { useNow } from "../workspace/clock";
import { describeAgent, relativeTime } from "../workspace/describe";

/**
 * What the agent of a session does: working, waiting for the user, done.
 * Empty while the session reports nothing (`agent` is `null`), which is
 * always the case until termx installs the hooks of Claude Code. A status
 * this version does not know shows as unknown: neutral, never loud.
 */
export function AgentSlot({ agent }: { agent: AgentState | null }) {
  const now = useNow();
  const text = describeAgent(agent);
  if (!text || !agent) {
    return <span className="agent-slot" data-testid="agent-slot" data-status="none" />;
  }
  const waiting =
    text.tone === "attention" && agent.waitingSince !== null
      ? ` · ${relativeTime(now, agent.waitingSince)}`
      : "";
  return (
    <span
      className="agent-slot"
      data-testid="agent-slot"
      data-status={text.unknown ? "unknown" : agent.status}
      data-tone={text.tone}
      title={`Agent: ${text.label}`}
    >
      {text.label}
      {waiting}
    </span>
  );
}
