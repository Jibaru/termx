/**
 * Small preferences of this window that the workspace does not hold: the
 * width of the sidebar, whether it is shown, which groups are collapsed.
 *
 * They live in the webview's local storage, which can be missing or refuse
 * to be written (private modes, a full disk): every access is guarded, and
 * a value that cannot be read is the default.
 */

const PREFIX = "termx.";

export function readPref<T>(key: string, fallback: T, valid: (value: unknown) => value is T): T {
  try {
    const raw = window.localStorage.getItem(PREFIX + key);
    if (raw === null) return fallback;
    const value: unknown = JSON.parse(raw);
    return valid(value) ? value : fallback;
  } catch {
    return fallback;
  }
}

export function writePref(key: string, value: unknown): void {
  try {
    window.localStorage.setItem(PREFIX + key, JSON.stringify(value));
  } catch {
    // Not remembered this time; nothing else depends on it.
  }
}

export const isNumber = (value: unknown): value is number =>
  typeof value === "number" && Number.isFinite(value);

export const isBoolean = (value: unknown): value is boolean => typeof value === "boolean";

export const isStringArray = (value: unknown): value is string[] =>
  Array.isArray(value) && value.every((item) => typeof item === "string");
