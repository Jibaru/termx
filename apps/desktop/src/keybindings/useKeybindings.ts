import { useEffect, useRef } from "react";

import { type Binding, type CommandId, matchBinding } from "./keybindings";

const REPEATABLE = new Set<CommandId>([
  "focusPaneLeft",
  "focusPaneRight",
  "focusPaneUp",
  "focusPaneDown",
  "nextSession",
  "previousSession",
]);

/** Whether the key event happens in a terminal (its hidden text area). */
export function isInTerminal(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest(".xterm") !== null;
}

/** Whether the key event happens in a field the user types in. */
export function isInTextField(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return (
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    target.isContentEditable
  );
}

/**
 * Runs the command of every shortcut pressed in the window.
 *
 * The listener is on the window, in the capture phase, so that it sees a
 * key before xterm.js does: a key that is a shortcut never reaches the
 * terminal, and every other key is left alone, untouched, for the program
 * in it. In a terminal only `global` bindings apply; in a text field
 * (renaming a session, the command of the new-session dialog) none does.
 * `enabled` is false while a dialog has the keyboard.
 */
export function useKeybindings(
  bindings: readonly Binding[],
  run: (command: CommandId, target: EventTarget | null) => void,
  enabled: boolean,
): void {
  const latest = useRef({ bindings, run, enabled });

  useEffect(() => {
    latest.current = { bindings, run, enabled };
  });

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const { bindings: current, run: handler, enabled: on } = latest.current;
      if (!on) return;
      const inTerminal = isInTerminal(event.target);
      // Typing in a field (a name, a command line) keeps every key.
      if (!inTerminal && isInTextField(event.target)) return;
      const command = matchBinding(current, event, inTerminal);
      if (command === null) return;
      event.preventDefault();
      event.stopPropagation();
      // Holding a key repeats moving around, never creating or closing.
      if (!event.repeat || REPEATABLE.has(command)) handler(command, event.target);
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
    };
  }, []);
}
