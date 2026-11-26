import { mockIPC } from "@tauri-apps/api/mocks";

import type { FrontendEvent } from "../lib/ipc";

export interface RecordedCall {
  command: string;
  args: unknown;
}

/**
 * Mocks the Tauri IPC and records every call. `respond` produces the result
 * of each command; throwing from it makes the call reject.
 */
export function recordIPC(respond: (command: string, args: unknown) => unknown = () => undefined) {
  const calls: RecordedCall[] = [];
  mockIPC((command, args) => {
    calls.push({ command, args });
    return respond(command, args);
  });

  return {
    calls,
    /** The records sent to the log so far. */
    logged(): FrontendEvent[] {
      return calls
        .filter((call) => call.command === "log_frontend_event")
        .map((call) => (call.args as { event: FrontendEvent }).event);
    },
  };
}
