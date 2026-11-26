import { mockIPC } from "@tauri-apps/api/mocks";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { App } from "./App";

describe("App", () => {
  it("renders the sidebar and the app name", () => {
    mockIPC(() => "0.0.0");

    render(<App />);

    expect(screen.getByRole("complementary", { name: "Sessions" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: "termx" })).toBeInTheDocument();
  });

  it("shows the core version returned by the core_version command", async () => {
    const calls: string[] = [];
    mockIPC((command) => {
      calls.push(command);
      return command === "core_version" ? "1.2.3" : undefined;
    });

    render(<App />);

    expect(await screen.findByText("core v1.2.3")).toBeInTheDocument();
    expect(calls).toContain("core_version");
  });

  it("reports when the core version cannot be fetched", async () => {
    mockIPC(() => {
      throw new Error("backend unavailable");
    });

    render(<App />);

    expect(await screen.findByText("core version unavailable")).toBeInTheDocument();
  });
});
