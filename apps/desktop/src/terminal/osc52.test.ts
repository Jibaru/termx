import { describe, expect, it } from "vitest";

import { clipboardTextFromOsc52, OSC52_MAX_BYTES } from "./osc52";

function base64(text: string): string {
  return btoa(String.fromCharCode(...new TextEncoder().encode(text)));
}

describe("clipboardTextFromOsc52", () => {
  it("decodes the text a program asks to copy", () => {
    expect(clipboardTextFromOsc52(`c;${base64("hello")}`)).toBe("hello");
    // An empty target list means the default clipboard.
    expect(clipboardTextFromOsc52(`;${base64("hello")}`)).toBe("hello");
  });

  it("decodes UTF-8", () => {
    expect(clipboardTextFromOsc52(`c;${base64("café ✓ 🚀")}`)).toBe("café ✓ 🚀");
  });

  it("clears the clipboard for an empty payload", () => {
    expect(clipboardTextFromOsc52("c;")).toBe("");
  });

  it("denies a request to read the clipboard", () => {
    expect(clipboardTextFromOsc52("c;?")).toBeNull();
    expect(clipboardTextFromOsc52(";?")).toBeNull();
  });

  it("accepts up to a megabyte and refuses more", () => {
    const largest = "x".repeat(OSC52_MAX_BYTES);

    expect(OSC52_MAX_BYTES).toBe(1024 * 1024);
    expect(clipboardTextFromOsc52(`c;${btoa(largest)}`)).toBe(largest);
    expect(clipboardTextFromOsc52(`c;${btoa(`${largest}xyz`)}`)).toBeNull();
  });

  it("ignores malformed requests", () => {
    expect(clipboardTextFromOsc52("no separator")).toBeNull();
    expect(clipboardTextFromOsc52("c;not base64!")).toBeNull();
  });
});
