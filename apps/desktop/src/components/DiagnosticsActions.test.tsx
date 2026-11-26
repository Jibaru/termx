import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { recordIPC } from "../test/ipcRecorder";
import { DiagnosticsActions } from "./DiagnosticsActions";

describe("DiagnosticsActions", () => {
  it("opens the log folder", async () => {
    const ipc = recordIPC();
    render(<DiagnosticsActions />);

    fireEvent.click(screen.getByRole("button", { name: "Open log folder" }));

    await screen.findByRole("button", { name: "Open log folder", busy: false });
    expect(ipc.calls.map((call) => call.command)).toEqual(["open_log_folder"]);
  });

  it("exports the diagnostics and shows where the bundle was saved", async () => {
    const ipc = recordIPC(() => ({ path: "/downloads/termx-diagnostics-1.zip" }));
    render(<DiagnosticsActions />);

    fireEvent.click(screen.getByRole("button", { name: "Export diagnostics" }));

    expect(await screen.findByRole("status")).toHaveTextContent(
      "Saved /downloads/termx-diagnostics-1.zip",
    );
    expect(ipc.calls).toEqual([{ command: "export_diagnostics", args: { destination: null } }]);
  });

  it("shows the reason when an action fails", async () => {
    recordIPC(() => {
      // eslint-disable-next-line @typescript-eslint/only-throw-error -- Tauri rejects with the serialised error.
      throw { command: "export_diagnostics", message: "logging is not running" };
    });
    render(<DiagnosticsActions />);

    fireEvent.click(screen.getByRole("button", { name: "Export diagnostics" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("logging is not running");
  });
});
