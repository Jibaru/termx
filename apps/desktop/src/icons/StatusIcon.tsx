import type { ReactNode } from "react";

import { Glyph, type IconSize } from "./Glyph";
import { type IconStatus, STATUS_TEXT } from "./statusText";

interface StatusIconProps {
  status: IconStatus;
  size?: IconSize;
  /** A tooltip more specific than the default, such as the failure message. */
  title?: string;
  /** Leave it out of the accessibility tree, when the words are next to it. */
  decorative?: boolean;
}

/**
 * The state of a session or of its agent, as a shape that is readable
 * without its colour:
 *
 * | State | Shape |
 * | --- | --- |
 * | starting | an open ring, turning slowly |
 * | running | a filled play mark in a ring |
 * | dormant | a crescent |
 * | exited cleanly | a check in a ring |
 * | exited with an error | a cross in a ring |
 * | failed | an octagon with an exclamation mark |
 * | warning | a triangle with an exclamation mark |
 * | working | three dots, pulsing in turn |
 * | waiting for permission | a padlock on a filled square: the loudest |
 * | waiting for input | a filled speech bubble with dots: as loud |
 * | done | a flag |
 * | unknown | a dashed ring with a question mark |
 *
 * Motion is slow and stops with `prefers-reduced-motion`.
 */
export function StatusIcon({ status, size = 16, title, decorative = false }: StatusIconProps) {
  const text = STATUS_TEXT[status];
  return (
    <Glyph
      shape={`status-${status}`}
      className="status-icon"
      tone={text.tone}
      size={size}
      label={decorative ? undefined : text.label}
      title={title ?? text.description}
    >
      {SHAPES[status]}
    </Glyph>
  );
}

const RING = <circle cx="8" cy="8" r="6" />;

const SHAPES: Record<IconStatus, ReactNode> = {
  starting: <path className="icon-spin" d="M8 2a6 6 0 1 1-6 6" />,
  running: (
    <>
      {RING}
      <path d="M6.5 5.25v5.5L10.75 8Z" fill="currentColor" strokeWidth="1.25" />
    </>
  ),
  dormant: <path d="M10.25 2.5A5.75 5.75 0 1 0 13.5 10 4.5 4.5 0 0 1 10.25 2.5Z" />,
  exitedOk: (
    <>
      {RING}
      <path d="m5.25 8.25 1.9 1.9 3.6-4" />
    </>
  ),
  exitedError: (
    <>
      {RING}
      <path d="m5.75 5.75 4.5 4.5m0-4.5-4.5 4.5" />
    </>
  ),
  failed: (
    <>
      <path d="M5.5 1.75h5l3.75 3.75v5l-3.75 3.75h-5L1.75 10.5v-5Z" />
      <path d="M8 4.75v3.75" />
      <path d="M8 11.25h.01" strokeWidth="2" />
    </>
  ),
  warning: (
    <>
      <path d="M8 1.75 14.5 13.5h-13Z" />
      <path d="M8 6v3.5" />
      <path d="M8 11.75h.01" strokeWidth="2" />
    </>
  ),
  working: (
    <g className="icon-pulse" fill="currentColor" stroke="none">
      <circle cx="3.5" cy="8" r="1.6" />
      <circle cx="8" cy="8" r="1.6" />
      <circle cx="12.5" cy="8" r="1.6" />
    </g>
  ),
  waitingForPermission: (
    <g className="icon-breathe">
      <rect x="1" y="1" width="14" height="14" rx="3.5" fill="currentColor" stroke="none" />
      <g stroke="var(--color-attention-text, #0a0a0a)" strokeWidth="1.5">
        <rect
          x="5"
          y="7.5"
          width="6"
          height="4.5"
          rx="1"
          fill="var(--color-attention-text, #0a0a0a)"
        />
        <path d="M6.25 7.5V6a1.75 1.75 0 0 1 3.5 0v1.5" />
      </g>
    </g>
  ),
  waitingForInput: (
    <g className="icon-breathe">
      <path
        d="M3.5 1.5h9A2.5 2.5 0 0 1 15 4v5a2.5 2.5 0 0 1-2.5 2.5H8L4.5 14.5V11.5h-1A2.5 2.5 0 0 1 1 9V4a2.5 2.5 0 0 1 2.5-2.5Z"
        fill="currentColor"
        strokeWidth="1"
      />
      <g fill="var(--color-attention-text, #0a0a0a)" stroke="none">
        <circle cx="5" cy="6.5" r="1.1" />
        <circle cx="8" cy="6.5" r="1.1" />
        <circle cx="11" cy="6.5" r="1.1" />
      </g>
    </g>
  ),
  done: (
    <>
      <path d="M4 14.25V2" />
      <path d="M4 2.5h8.25L10.5 5.25l1.75 2.75H4" fill="currentColor" />
    </>
  ),
  unknown: (
    <>
      <circle cx="8" cy="8" r="6" strokeDasharray="2.4 2.3" />
      <path d="M6.25 6.25a1.75 1.75 0 1 1 2.5 1.6c-.5.25-.75.6-.75 1.15" />
      <path d="M8 11.25h.01" strokeWidth="2" />
    </>
  ),
};
