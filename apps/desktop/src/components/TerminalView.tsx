import "@xterm/xterm/css/xterm.css";

import { type CSSProperties, useEffect, useRef } from "react";

import { DEFAULT_TERMINAL_CONFIG, type TerminalConfig } from "../terminal/config";
import type { TerminalController } from "../terminal/TerminalController";

interface TerminalViewProps {
  controller: TerminalController;
  /** Where the padding and the background of the pane come from. */
  config?: TerminalConfig;
}

/**
 * The pane a terminal is drawn in. It only lends its element to the
 * controller, which owns the xterm.js instance and its session: switching
 * to another terminal swaps what is in the pane without touching either.
 */
export function TerminalView({ controller, config = DEFAULT_TERMINAL_CONFIG }: TerminalViewProps) {
  const host = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const element = host.current;
    if (!element) return;
    controller.attach(element);
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
