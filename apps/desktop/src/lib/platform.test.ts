import { describe, expect, it } from "vitest";

import { detectPlatform } from "./platform";

describe("detectPlatform", () => {
  it("recognises the webview of each system from its user agent", () => {
    expect(
      detectPlatform({
        userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15",
      }),
    ).toBe("macos");
    expect(
      detectPlatform({
        userAgent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Edg/135.0",
      }),
    ).toBe("windows");
    expect(
      detectPlatform({ userAgent: "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15" }),
    ).toBe("linux");
  });
});
