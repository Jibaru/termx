/**
 * Descriptions of thrown values that are safe to write to the log file.
 *
 * Anything can be thrown or used to reject a promise in JavaScript, and the
 * value may carry data that must never be logged (a response body, a token,
 * an environment). So:
 *
 * - for an `Error`, only its name, message and stack are logged, never its
 *   other properties;
 * - for anything else, only the type and, for objects, the names of the
 *   top-level keys are logged. Values are never logged, not even strings.
 */

export interface LoggableError {
  message: string;
  stack?: string;
}

/** Longest key name and most keys listed for a thrown object. */
const MAX_KEY_LENGTH = 40;
const MAX_KEYS = 20;

/** The type of `value` and, for objects, its top-level key names. */
export function describeShape(value: unknown): string {
  if (value === null) return "null";
  if (Array.isArray(value)) return `array of length ${String(value.length)}`;
  switch (typeof value) {
    case "string":
      return `string of length ${String(value.length)}`;
    case "object": {
      let keys: string[];
      try {
        keys = Object.keys(value);
      } catch {
        return "object";
      }
      const shown = keys.slice(0, MAX_KEYS).map((key) => key.slice(0, MAX_KEY_LENGTH));
      const more = keys.length > MAX_KEYS ? `, and ${String(keys.length - MAX_KEYS)} more` : "";
      return `object with keys [${shown.join(", ")}${more}]`;
    }
    default:
      return typeof value;
  }
}

/** What may be logged about something that was thrown or rejected. */
export function describeForLog(thrown: unknown): LoggableError {
  if (thrown instanceof Error) {
    return {
      message: thrown.message ? `${thrown.name}: ${thrown.message}` : thrown.name,
      stack: typeof thrown.stack === "string" ? thrown.stack : undefined,
    };
  }
  return { message: `non-Error value (${describeShape(thrown)})` };
}

/**
 * What may be logged about a failed IPC call.
 *
 * Tauri rejects with a plain string for failures of the call itself. The
 * string can quote the arguments (a deserialisation error does), so it is
 * only classified, not copied.
 */
export function describeIpcFailure(thrown: unknown): LoggableError {
  if (typeof thrown !== "string") return describeForLog(thrown);

  let kind = "unrecognised failure";
  if (/not found/i.test(thrown)) kind = "command not found";
  else if (/not allowed|permission|denied|forbidden/i.test(thrown)) kind = "not allowed";
  else if (/invalid args|invalid type|missing required|deserializ/i.test(thrown))
    kind = "invalid arguments";
  return { message: `${kind} (${describeShape(thrown)})` };
}
