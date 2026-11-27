import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { flush, renderApp } from "../test/fakeWorkspace";
import { session, workspace } from "../test/workspaceFixtures";

const recent = ["/work/app", "/work/api"];

async function open(responders: Parameters<typeof renderApp>[1] = {}) {
  const view = await renderApp(
    workspace([session("s-1", { state: { status: "running" } })], { recentFolders: recent }),
    responders,
  );
  fireEvent.click(screen.getByTestId("new-session-button"));
  const dialog = screen.getByTestId("new-session-dialog");
  return { ...view, dialog };
}

function folders(): string[] {
  return screen
    .getAllByTestId("folder-option")
    .map((option) => option.querySelector(".folder-name")?.textContent ?? "");
}

describe("the new-session dialog", () => {
  it("starts Claude Code in the most recent folder on Enter", async () => {
    const view = await open();

    expect(folders()).toEqual(["app", "api"]);
    const list = screen.getByTestId("folder-list");
    expect(document.activeElement).toBe(list);
    expect(screen.getByTestId("kind-claude")).toBeChecked();
    fireEvent.keyDown(list, { key: "Enter" });
    await flush();

    expect(view.callsOf("createSession")).toEqual([
      [{ kind: "claude", cwd: "/work/app", show: true }],
    ]);
    expect(screen.queryByTestId("new-session-dialog")).toBeNull();
  });

  it("chooses another recent folder with the arrow keys", async () => {
    const view = await open();
    const list = screen.getByTestId("folder-list");

    fireEvent.keyDown(list, { key: "ArrowDown" });
    expect(screen.getAllByTestId("folder-option")[1]).toHaveAttribute("aria-selected", "true");
    fireEvent.click(screen.getByTestId("kind-shell"));
    fireEvent.click(screen.getByTestId("create-session"));
    await flush();

    expect(view.callsOf("createSession")).toEqual([
      [{ kind: "shell", cwd: "/work/api", show: true }],
    ]);
  });

  it("takes a recent folder off the list, with undo, and forgets it when it closes", async () => {
    const view = await open();

    const [first] = screen.getAllByTestId("folder-option");
    if (!first) throw new Error("no folder");
    fireEvent.click(within(first).getByTestId("folder-forget"));
    expect(folders()).toEqual(["api"]);
    expect(screen.getByTestId("folder-removed")).toHaveTextContent(
      "Removed app from the recent folders.",
    );
    // Nothing is forgotten while it can still be undone.
    expect(view.callsOf("forgetRecentFolder")).toEqual([]);
    fireEvent.click(screen.getByTestId("folder-undo"));
    expect(folders()).toEqual(["app", "api"]);

    fireEvent.keyDown(screen.getByTestId("folder-list"), { key: "Delete" });
    expect(folders()).toEqual(["api"]);
    fireEvent.keyDown(screen.getByTestId("folder-list"), { key: "Escape" });
    await flush();

    expect(view.callsOf("forgetRecentFolder")).toEqual([["/work/app"]]);
  });

  it("does not take a folder off the list on Backspace", async () => {
    const view = await open();

    fireEvent.keyDown(screen.getByTestId("folder-list"), { key: "Backspace" });
    fireEvent.keyDown(screen.getByTestId("folder-list"), { key: "Escape" });
    await flush();

    expect(view.callsOf("forgetRecentFolder")).toEqual([]);
  });

  it("takes a folder from the picker of the system, and puts it first", async () => {
    const view = await open({ responders: { pickFolder: () => "/elsewhere/tool" } });

    fireEvent.click(screen.getByTestId("choose-folder"));
    await flush();
    expect(folders()).toEqual(["tool", "app", "api"]);
    expect(screen.getAllByTestId("folder-option")[0]).toHaveAttribute("aria-selected", "true");
    fireEvent.click(screen.getByTestId("create-session"));
    await flush();

    expect(view.callsOf("createSession")).toEqual([
      [{ kind: "claude", cwd: "/elsewhere/tool", show: true }],
    ]);
  });

  it("changes nothing when the picker is dismissed", async () => {
    await open({ responders: { pickFolder: () => null } });

    fireEvent.click(screen.getByTestId("choose-folder"));
    await flush();

    expect(folders()).toEqual(["app", "api"]);
  });

  it("needs a command for a background session, and takes an optional name", async () => {
    const view = await open();

    fireEvent.click(screen.getByTestId("kind-background"));
    expect(screen.getByTestId("create-session")).toBeDisabled();
    fireEvent.keyDown(screen.getByTestId("folder-list"), { key: "Enter" });
    expect(screen.getByTestId("new-session-error")).toHaveTextContent(
      "A background session needs a command to run.",
    );
    expect(view.callsOf("createSession")).toEqual([]);

    fireEvent.change(screen.getByTestId("background-command"), {
      target: { value: " npm run dev " },
    });
    fireEvent.change(screen.getByTestId("background-name"), { target: { value: "Dev server" } });
    fireEvent.click(screen.getByTestId("create-session"));
    await flush();

    expect(view.callsOf("createSession")).toEqual([
      [
        {
          kind: "background",
          cwd: "/work/app",
          command: "npm run dev",
          name: "Dev server",
          show: true,
        },
      ],
    ]);
  });

  it.each([
    ["folderMissing", "Folder no longer exists"],
    ["claudeNotFound", "Claude Code is not installed"],
    ["invalidCommand", "The command is not valid"],
  ])("says why the session could not be created (%s), and stays open", async (kind, words) => {
    await open({
      responders: {
        createSession: () => {
          // eslint-disable-next-line @typescript-eslint/only-throw-error -- Tauri rejects with the serialised error.
          throw { command: "session_create", kind, message: "" };
        },
      },
    });

    fireEvent.click(screen.getByTestId("create-session"));

    await waitFor(() => {
      expect(screen.getByTestId("new-session-error")).toHaveTextContent(words);
    });
    expect(screen.getByTestId("new-session-dialog")).toBeInTheDocument();
    expect(screen.getByTestId("create-session")).toBeEnabled();
  });

  it("offers the home folder when there are no recent folders", async () => {
    const view = await renderApp(workspace([session("s-1", { state: { status: "running" } })]));
    fireEvent.click(screen.getByTestId("new-session-button"));

    expect(folders()).toEqual(["Home folder"]);
    fireEvent.click(screen.getByTestId("create-session"));
    await flush();

    expect(view.callsOf("createSession")).toEqual([[{ kind: "claude", show: true }]]);
  });

  it("closes on Escape without creating anything", async () => {
    const view = await open();

    fireEvent.keyDown(screen.getByTestId("folder-list"), { key: "Escape" });

    expect(screen.queryByTestId("new-session-dialog")).toBeNull();
    expect(view.callsOf("createSession")).toEqual([]);
  });

  it("keeps the shortcuts of the window away while it is open", async () => {
    const view = await open();

    fireEvent.keyDown(window, { code: "KeyN", ctrlKey: true, shiftKey: true });
    await flush();

    expect(view.callsOf("createSiblingSession")).toEqual([]);
  });
});
