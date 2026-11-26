/**
 * OSC 52: the escape sequence with which a program asks the terminal to
 * access the clipboard. Its payload is `<targets>;<data>`.
 *
 * termx lets programs **write** the clipboard, which is how copying works
 * from a program that runs on another machine or inside tmux, and how
 * Claude Code's `/copy` reaches the system clipboard. It never lets them
 * **read** it (`<data>` is `?`): what the user copied elsewhere is none of
 * the business of whatever happens to print to the terminal.
 */

/**
 * Largest text a program may put on the clipboard, in bytes of UTF-8.
 *
 * A megabyte is far more than anyone copies out of a terminal on purpose,
 * and small enough that a program cannot make termx decode and hand to the
 * system an arbitrarily large payload behind the user's back (xterm.js
 * itself accepts sequences of up to 10 MB).
 */
export const OSC52_MAX_BYTES = 1024 * 1024;

/** Base64 encodes every 3 bytes as 4 characters. */
const OSC52_MAX_BASE64_LENGTH = Math.ceil(OSC52_MAX_BYTES / 3) * 4;

/**
 * The text an OSC 52 payload asks to put on the clipboard, or `null` when
 * the request is not honoured: a read, data that is not valid base64, or
 * more than `OSC52_MAX_BYTES`.
 */
export function clipboardTextFromOsc52(payload: string): string | null {
  const separator = payload.indexOf(";");
  if (separator < 0) return null;
  const data = payload.slice(separator + 1);
  // A request to read the clipboard: denied.
  if (data === "?") return null;
  // Checked before decoding, so that an oversized payload costs nothing.
  if (data.length > OSC52_MAX_BASE64_LENGTH) return null;

  let binary: string;
  try {
    binary = atob(data);
  } catch {
    return null;
  }
  const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
  return new TextDecoder().decode(bytes);
}
