/**
 * Descriptions of thrown values that are safe to write to the log file.
 *
 * Anything can be thrown or used to reject a promise in JavaScript, and the
 * value may carry data that must never be logged (a response body, a token,
 * an environment). So:
 *
 * - for an `Error`, only its name, message and stack are logged, never its
 *   other properties;
 * - for anything else, only the type and, for objects, the number of
 *   top-level keys and the names of those that are plainly field names are
 *   logged. Values are never logged, not even strings.
 *
 * The Rust side also removes what looks like a secret from every record
 * (`scrub.rs`), messages and stacks of errors included.
 */

export interface LoggableError {
  message: string;
  stack?: string;
}

/** Most key names listed for a thrown object. */
const MAX_KEYS = 20;

/**
 * A key is only named when it looks like a field name written by a
 * programmer: letters and underscores, at most 24 of them, optionally ending
 * in one or two digits. Anything else can be data (a token used as a key, an
 * e-mail address, a path) and is only counted.
 */
const FIELD_NAME = /^[A-Za-z_$][A-Za-z_$]{0,23}\d{0,2}$/;

/** The type of `value` and, for objects, its number of keys and the names of
 * those that are plainly field names. */
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
      const names = keys.filter((key) => FIELD_NAME.test(key));
      const shown = names.slice(0, MAX_KEYS);
      const hidden = keys.length - shown.length;
      const list = hidden > 0 ? [...shown, `+${String(hidden)} not shown`] : shown;
      const count = `${String(keys.length)} ${keys.length === 1 ? "key" : "keys"}`;
      return `object with ${count} [${list.join(", ")}]`;
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
