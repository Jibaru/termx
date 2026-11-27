import type { ReactNode } from "react";

/**
 * The frame of every icon of termx: one visual language for all of them.
 *
 * - A 16 by 16 grid, drawn at 12, 16 or 20 CSS pixels. Strokes are 1.5
 *   units with round caps and joins, and sit on the grid so that they stay
 *   crisp on standard and high-density screens alike.
 * - `currentColor` for the line, which the palette sets through the
 *   `data-tone` of the icon (see `styles.css`); a shape may be filled with
 *   the same colour as its accent.
 * - With a `label`, the icon is an image with that name and a tooltip
 *   (`<title>`); without one it is decoration, hidden from screen readers.
 * - `shape` names the drawing, the same for every size and colour: what
 *   tells two states apart is the shape, not only the colour.
 */

export type IconSize = 12 | 16 | 20;

export interface GlyphProps {
  /** What the icon means, for screen readers and its tooltip. */
  label?: string;
  /** A longer tooltip than the label. */
  title?: string;
  size?: IconSize;
  className?: string;
  /** Which colour of the palette it is drawn in. */
  tone?: string;
}

export function Glyph({
  shape,
  label,
  title,
  size = 16,
  className,
  tone,
  children,
}: GlyphProps & { shape: string; children: ReactNode }) {
  const tooltip = title ?? label;
  return (
    <svg
      className={className ? `icon ${className}` : "icon"}
      data-shape={shape}
      data-tone={tone}
      viewBox="0 0 16 16"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      focusable="false"
      {...(label ? { role: "img", "aria-label": label } : { "aria-hidden": true })}
    >
      {tooltip && label ? <title>{tooltip}</title> : null}
      {children}
    </svg>
  );
}
