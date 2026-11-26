import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { recordIPC } from "../test/ipcRecorder";
import { ErrorBoundary } from "./ErrorBoundary";

let shouldThrow = true;

function Flaky() {
  if (shouldThrow) throw new Error("cannot render the widget");
  return <p>widget</p>;
}

beforeEach(() => {
  shouldThrow = true;
  // React prints every error caught by a boundary.
  vi.spyOn(console, "error").mockImplementation(() => undefined);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("ErrorBoundary", () => {
  it("renders its children when nothing fails", () => {
    recordIPC();
    shouldThrow = false;

    render(
      <ErrorBoundary>
        <Flaky />
      </ErrorBoundary>,
    );

    expect(screen.getByText("widget")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("shows a readable fallback with the error and the diagnostics actions", () => {
    recordIPC();

    render(
      <ErrorBoundary>
        <Flaky />
      </ErrorBoundary>,
    );

    expect(screen.getByRole("heading", { name: "Something went wrong" })).toBeInTheDocument();
    expect(screen.getByText("cannot render the widget")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open log folder" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Export diagnostics" })).toBeInTheDocument();
  });

  it("logs the error with the component stack", async () => {
    const ipc = recordIPC();

    render(
      <ErrorBoundary>
        <Flaky />
      </ErrorBoundary>,
    );

    await vi.waitFor(() => {
      expect(ipc.logged()).toHaveLength(1);
    });
    const [event] = ipc.logged();
    expect(event).toMatchObject({
      level: "error",
      source: "react",
      message: "Error: cannot render the widget",
    });
    expect(event?.stack).toContain("cannot render the widget");
    expect(event?.componentStack).toContain("Flaky");
  });

  it("renders the children again after Try again", () => {
    recordIPC();
    render(
      <ErrorBoundary>
        <Flaky />
      </ErrorBoundary>,
    );
    shouldThrow = false;

    fireEvent.click(screen.getByRole("button", { name: "Try again" }));

    expect(screen.getByText("widget")).toBeInTheDocument();
  });
});
