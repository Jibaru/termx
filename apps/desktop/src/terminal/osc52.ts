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
 * The text an OSC 52 payload asks to put on the clipboard, or `null` when
 * the request is not honoured: a read, or data that is not valid base64.
 */
export function clipboardTextFromOsc52(payload: string): string | null {
  const separator = payload.indexOf(";");
  if (separator < 0) return null;
  const data = payload.slice(separator + 1);
  // A request to read the clipboard: denied.
  if (data === "?") return null;

  let binary: string;
  try {
    binary = atob(data);
  } catch {
    return null;
  }
  const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
  return new TextDecoder().decode(bytes);
}
