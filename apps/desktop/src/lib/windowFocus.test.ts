import { emit } from "@tauri-apps/api/event";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { afterEach, describe, expect, it, vi } from "vitest";

import { windowFocusSource } from "./windowFocus";

function setVisibility(state: DocumentVisibilityState) {
  Object.defineProperty(document, "visibilityState", { configurable: true, value: state });
  document.dispatchEvent(new Event("visibilitychange"));
}

afterEach(() => {
  setVisibility("visible");
});

describe("the focus of the window", () => {
  it("follows the focus events of Tauri and the visibility of the page", async () => {
    mockWindows("main");
    mockIPC(
      (command) => {
        if (command === "plugin:window|is_focused") return false;
        return undefined;
      },
      { shouldMockEvents: true },
    );
    const focus = windowFocusSource();
    const changes = vi.fn();
    focus.subscribe(changes);
    await vi.waitFor(() => {
      expect(focus.focused()).toBe(false);
    });

    await emit("tauri://focus");
    expect(focus.focused()).toBe(true);

    // Minimised, or on another space: not looked at, even with the focus.
    setVisibility("hidden");
    expect(focus.focused()).toBe(false);
    setVisibility("visible");
    expect(focus.focused()).toBe(true);

    await emit("tauri://blur");
    expect(focus.focused()).toBe(false);
    expect(changes).toHaveBeenCalledTimes(5);
  });
});
