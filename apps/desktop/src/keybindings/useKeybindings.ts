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
function isInTextField(target: EventTarget | null): boolean {
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
 * in it. In a terminal or a text field only `global` bindings apply.
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
      const restricted = isInTerminal(event.target) || isInTextField(event.target);
      const command = matchBinding(current, event, restricted);
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
