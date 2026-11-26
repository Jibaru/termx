import { PNG } from "pngjs";

import type { PageLayout, Rect } from "./page.ts";

/** A pixel counts as drawn when a channel differs this much from the background. */
export const CHANNEL_TOLERANCE = 48;

export type Colour = [red: number, green: number, blue: number];

/** Parses `rgb(10, 10, 10)` or `rgba(10, 10, 10, 1)`, as CSS reports colours. */
export function parseColour(css: string): Colour {
  const channels = css.match(/\d+(\.\d+)?/g)?.map(Number) ?? [];
  const [red, green, blue] = channels;
  if (red === undefined || green === undefined || blue === undefined) {
    throw new Error(`cannot read the colour ${css}`);
  }
  return [red, green, blue];
}

export interface Coverage {
  /** Pixels of the area that are not the background colour. */
  drawn: number;
  /** Pixels of the area. */
  total: number;
}

/**
 * Counts the pixels of a PNG image that are not the background colour:
 * text, the cursor, anything drawn. `area` limits the count to a part of
 * the page, given in CSS pixels as `layout` reports it; without it the
 * whole image counts.
 *
 * The comparison has a tolerance and the result is a count, not an image
 * to compare: fonts, antialiasing and scaling differ on every machine.
 */
export function coverage(png: Buffer, layout: PageLayout, area?: Rect): Coverage {
  const image = PNG.sync.read(png);
  const [red, green, blue] = parseColour(layout.background);
  // Screenshots are in device pixels, the layout in CSS pixels.
  const scale = image.width / layout.viewportWidth;

  const left = area ? Math.max(0, Math.floor(area.x * scale)) : 0;
  const top = area ? Math.max(0, Math.floor(area.y * scale)) : 0;
  const right = area
    ? Math.min(image.width, Math.ceil((area.x + area.width) * scale))
    : image.width;
  const bottom = area
    ? Math.min(image.height, Math.ceil((area.y + area.height) * scale))
    : image.height;

  let drawn = 0;
  for (let y = top; y < bottom; y += 1) {
    for (let x = left; x < right; x += 1) {
      const offset = (y * image.width + x) * 4;
      const difference = Math.max(
        Math.abs((image.data[offset] ?? 0) - red),
        Math.abs((image.data[offset + 1] ?? 0) - green),
        Math.abs((image.data[offset + 2] ?? 0) - blue),
      );
      if (difference > CHANNEL_TOLERANCE) drawn += 1;
    }
  }
  return { drawn, total: Math.max(0, right - left) * Math.max(0, bottom - top) };
}
