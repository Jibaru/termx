import { describe, expect, it } from "vitest";

import { SessionStore } from "../terminal/sessionStore";
import { installE2eHook } from "./e2eHook";

function fakeWindow(marker?: true): Window {
  return { __TERMX_E2E__: marker } as unknown as Window;
}

describe("installE2eHook", () => {
  it("exposes nothing in a build that is not made for the end-to-end suite", () => {
    const target = fakeWindow();

    installE2eHook(new SessionStore(), target);

    expect(target.__TERMX_E2E__).toBeUndefined();
  });

  it("exposes the terminals when the Rust side has set the marker", () => {
    const target = fakeWindow(true);
    const store = new SessionStore();

    installE2eHook(store, target);

    expect(target.__TERMX_E2E__).toEqual({ store });
  });
});
