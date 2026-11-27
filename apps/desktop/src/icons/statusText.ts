/**
 * The states an icon can show, with their names and what their tooltip
 * says. The shapes are in `StatusIcon.tsx`.
 */

/** What the process of a session is doing. */
export type ProcessStatus =
  "starting" | "running" | "dormant" | "exitedOk" | "exitedError" | "failed" | "warning";

/** What the agent of a session is doing (filled in by milestone 3). */
export type AgentIconStatus =
  "working" | "waitingForPermission" | "waitingForInput" | "done" | "unknown";

export type IconStatus = ProcessStatus | AgentIconStatus;

export interface StatusText {
  label: string;
  description: string;
  /** The colour of the palette it is drawn in. */
  tone: "info" | "success" | "muted" | "danger" | "warning" | "attention";
}

export const STATUS_TEXT: Record<IconStatus, StatusText> = {
  starting: {
    label: "Starting",
    description: "Starting: the program is being launched.",
    tone: "info",
  },
  running: { label: "Running", description: "Running.", tone: "success" },
  dormant: {
    label: "Not running",
    description: "Not running: restored from the last run, or stopped. Resume starts it again.",
    tone: "muted",
  },
  exitedOk: { label: "Exited", description: "Exited: the program ended normally.", tone: "muted" },
  exitedError: {
    label: "Exited with an error",
    description: "Exited with an error code.",
    tone: "danger",
  },
  failed: {
    label: "Failed",
    description: "Failed: termx could not start it, or knows why it stopped.",
    tone: "danger",
  },
  warning: {
    label: "Running with a warning",
    description: "Running, but something is off.",
    tone: "warning",
  },
  working: { label: "Working", description: "The agent is working.", tone: "info" },
  waitingForPermission: {
    label: "Needs permission",
    description: "The agent cannot go on until you allow what it asks.",
    tone: "attention",
  },
  waitingForInput: {
    label: "Waiting for input",
    description: "The agent waits for your answer.",
    tone: "attention",
  },
  done: { label: "Done", description: "The agent finished.", tone: "success" },
  unknown: {
    label: "Unknown",
    description: "The agent reported a state this version of termx does not know.",
    tone: "muted",
  },
};
