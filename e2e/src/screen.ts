import { execFileSync } from "node:child_process";

import type { WebDriver } from "./webdriver.ts";

/** A picture of the application as it reaches the screen, as a PNG file. */
export interface ScreenCapture {
  png: Buffer;
  /** Whether the picture is the page alone, or everything on the display. */
  of: "page" | "display";
}

/**
 * Takes a picture of the application as it reaches the screen, where the
 * suite has a way to take one that can be trusted.
 *
 * - **macOS and Windows**: the screenshot of the webview, which includes
 *   what WebGL draws.
 * - **Linux**: the screenshot of WebKitGTK leaves out what WebGL draws, so
 *   the display itself is captured (`import`, from ImageMagick). That is
 *   only done in CI, where the display is a virtual one that shows nothing
 *   but the application. Elsewhere there is no picture.
 */
export async function captureScreen(driver: WebDriver): Promise<ScreenCapture | undefined> {
  if (process.platform !== "linux") return { png: await driver.screenshot(), of: "page" };
  if (process.env.CI === undefined) return undefined;
  const png = execFileSync("import", ["-window", "root", "png:-"], {
    maxBuffer: 64 * 1024 * 1024,
  });
  return { png, of: "display" };
}
