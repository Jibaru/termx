import { mockIPC } from "@tauri-apps/api/mocks";
import { act, render, screen } from "@testing-library/react";
import { StrictMode } from "react";
import { describe, expect, it, vi } from "vitest";

import { App } from "./App";
import { SessionStore } from "./terminal/sessionStore";
import type { TerminalController } from "./terminal/TerminalController";

/** A store whose terminals record where they were shown instead of drawing. */
function storeOfFakeTerminals() {
  const controllers: { attach: ReturnType<typeof vi.fn>; detach: ReturnType<typeof vi.fn> }[] = [];
  const store = new SessionStore(() => {
    const controller = { attach: vi.fn(), detach: vi.fn(), focus: vi.fn(), dispose: vi.fn() };
    controllers.push(controller);
    return controller as unknown as TerminalController;
  });
  return { store, controllers };
}

describe("App", () => {
  it("renders the sidebar and the app name", () => {
    mockIPC(() => "0.0.0");

    render(<App store={storeOfFakeTerminals().store} />);

    expect(screen.getByRole("complementary", { name: "Sessions" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: "termx" })).toBeInTheDocument();
  });

  it("shows the core version returned by the core_version command", async () => {
    const calls: string[] = [];
    mockIPC((command) => {
      calls.push(command);
      return command === "core_version" ? "1.2.3" : undefined;
    });

    render(<App store={storeOfFakeTerminals().store} />);

    expect(await screen.findByText("core v1.2.3")).toBeInTheDocument();
    expect(calls).toContain("core_version");
  });

  it("reports when the core version cannot be fetched", async () => {
    mockIPC(() => {
      throw new Error("backend unavailable");
    });

    render(<App store={storeOfFakeTerminals().store} />);

    expect(await screen.findByText("core version unavailable")).toBeInTheDocument();
  });

  it("keeps the diagnostics actions within reach", () => {
    mockIPC(() => "0.0.0");

    render(<App store={storeOfFakeTerminals().store} />);

    expect(screen.getByRole("button", { name: "Open log folder" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Export diagnostics" })).toBeInTheDocument();
  });

  it("opens one terminal and shows it in the main region", () => {
    mockIPC(() => "0.0.0");
    const { store, controllers } = storeOfFakeTerminals();

    render(<App store={store} />);

    expect(controllers).toHaveLength(1);
    const main = screen.getByRole("main", { name: "Terminal" });
    const host = main.querySelector(".terminal-host");
    expect(host).not.toBeNull();
    expect(controllers[0]?.attach).toHaveBeenCalledExactlyOnceWith(host);
  });

  it("opens a single terminal even when effects run twice", () => {
    mockIPC(() => "0.0.0");
    const { store, controllers } = storeOfFakeTerminals();

    render(
      <StrictMode>
        <App store={store} />
      </StrictMode>,
    );

    // Strict mode mounts, unmounts and mounts again: the terminal is taken
    // off the screen and put back, not opened twice.
    expect(controllers).toHaveLength(1);
    expect(store.getSnapshot().keys).toHaveLength(1);
    expect(controllers[0]?.attach).toHaveBeenCalledTimes(2);
    expect(controllers[0]?.detach).toHaveBeenCalledTimes(1);
  });

  it("shows only the active terminal, and switches without disposing of any", () => {
    mockIPC(() => "0.0.0");
    const { store, controllers } = storeOfFakeTerminals();
    const { unmount } = render(<App store={store} />);
    const first = store.getSnapshot().activeKey ?? "";

    let second = "";
    act(() => {
      second = store.open();
    });
    expect(controllers).toHaveLength(2);
    expect(controllers[0]?.detach).toHaveBeenCalledTimes(1);
    expect(controllers[1]?.attach).toHaveBeenCalledTimes(1);
    expect(document.querySelectorAll(".terminal-host")).toHaveLength(1);

    act(() => {
      store.activate(first);
    });
    expect(controllers[1]?.detach).toHaveBeenCalledTimes(1);
    expect(controllers[0]?.attach).toHaveBeenCalledTimes(2);
    expect(store.get(second)).toBe(controllers[1]);

    unmount();
  });
});
