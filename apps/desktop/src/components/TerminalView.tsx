import "@xterm/xterm/css/xterm.css";

import { type CSSProperties, useEffect, useRef } from "react";

import { DEFAULT_TERMINAL_CONFIG, type TerminalConfig } from "../terminal/config";
import type { TerminalController } from "../terminal/TerminalController";

interface TerminalViewProps {
  controller: TerminalController;
  /** Where the padding and the background of the pane come from. */
  config?: TerminalConfig;
  /** Whether the terminal takes the keyboard when it is shown. Yes by default. */
  autoFocus?: boolean;
}

/**
 * The pane a terminal is drawn in. It only lends its element to the
 * controller, which owns the xterm.js instance and its session: switching
 * to another terminal swaps what is in the pane without touching either.
 */
export function TerminalView({
  controller,
  config = DEFAULT_TERMINAL_CONFIG,
  autoFocus = true,
}: TerminalViewProps) {
  const host = useRef<HTMLDivElement>(null);
  // Read when the terminal is shown, not a reason to show it again.
  const focusOnAttach = useRef(autoFocus);

  // Declared first, so it runs before the effect that reads it.
  useEffect(() => {
    focusOnAttach.current = autoFocus;
  });

  useEffect(() => {
    const element = host.current;
    if (!element) return;
    controller.attach(element, { focus: focusOnAttach.current });
    return () => {
      controller.detach();
    };
  }, [controller]);

  const style = {
    "--terminal-background": config.theme.background,
    "--terminal-padding-x": `${String(config.padding.x)}px`,
    "--terminal-padding-y": `${String(config.padding.y)}px`,
  } as CSSProperties;

  return (
    <div
      ref={host}
      className="terminal-host"
      data-testid="terminal-host"
      style={style}
      // A click on the padding around the terminal still gives it the keys.
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) {
          event.preventDefault();
          controller.focus();
        }
      }}
    />
  );
}
