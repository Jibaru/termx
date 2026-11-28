import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { manualFocusSource } from "../lib/windowFocus";
import { fakeClaude, INSTALLED } from "../test/fakeClaude";
import { IntegrationUnavailableError } from "./backend";
import { ClaudeIntegrationStore, describeIntegrationError, RETRY_MS } from "./integration";

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("reading the integration", () => {
  it("tries again after a failure, waiting longer each time", async () => {
    let attempts = 0;
    const fake = fakeClaude(INSTALLED, {
      getClaudeIntegration: () => {
        attempts += 1;
        if (attempts <= 2) throw new Error("the socket is not ready");
        return INSTALLED;
      },
    });
    const store = new ClaudeIntegrationStore(fake.backend);
    store.start(manualFocusSource());
    await vi.advanceTimersByTimeAsync(0);

    expect(store.getSnapshot().state).toEqual({ status: "failed" });
    await vi.advanceTimersByTimeAsync(RETRY_MS[0]);
    expect(attempts).toBe(2);
    await vi.advanceTimersByTimeAsync(RETRY_MS[1] - 1);
    expect(attempts).toBe(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(attempts).toBe(3);
    expect(store.reporting).toBe(true);
    // Read once it worked: no more retries.
    await vi.advanceTimersByTimeAsync(RETRY_MS.at(-1) ?? 0);
    expect(attempts).toBe(3);
    store.stop();
  });

  it("reads it again when the window gets the focus back", async () => {
    const fake = fakeClaude({ claudeInstalled: false, plugin: "notInstalled", consent: "unasked" });
    const focus = manualFocusSource();
    const store = new ClaudeIntegrationStore(fake.backend);
    store.start(focus);
    await vi.advanceTimersByTimeAsync(0);
    expect(store.shouldAsk).toBe(false);

    // Claude Code installed meanwhile, in another window.
    fake.setState({ claudeInstalled: true });
    focus.set(false);
    focus.set(true);
    await vi.advanceTimersByTimeAsync(0);

    expect(store.shouldAsk).toBe(true);
    expect(fake.callsOf("getClaudeIntegration")).toHaveLength(2);
    store.stop();
  });

  it("reads it again when the Rust side says it changed", async () => {
    const fake = fakeClaude();
    const store = new ClaudeIntegrationStore(fake.backend);
    store.start(manualFocusSource());
    await vi.advanceTimersByTimeAsync(0);
    expect(store.reporting).toBe(true);

    // The plugin was removed by hand: agent states are no longer trusted.
    fake.setState({ plugin: "notInstalled" });
    fake.announceChange();
    await vi.advanceTimersByTimeAsync(0);

    expect(store.reporting).toBe(false);
    store.stop();
  });

  it("is unavailable, quietly and for good, in a window without the integration", async () => {
    const fake = fakeClaude(INSTALLED, {
      getClaudeIntegration: () => {
        throw new IntegrationUnavailableError();
      },
    });
    const store = new ClaudeIntegrationStore(fake.backend);
    store.start(manualFocusSource());
    await vi.advanceTimersByTimeAsync(RETRY_MS.at(-1) ?? 0);

    expect(store.getSnapshot().state).toEqual({ status: "unavailable" });
    expect(fake.callsOf("getClaudeIntegration")).toHaveLength(1);
    store.stop();
  });
});

describe("errors", () => {
  it("use their own words by kind, and the message for a kind not known", () => {
    expect(describeIntegrationError({ kind: "storageFull", message: "" })).toBe(
      "The disk is full.",
    );
    // Not a property of every object.
    expect(describeIntegrationError({ kind: "constructor" as never, message: "odd" })).toBe("odd");
  });
});
