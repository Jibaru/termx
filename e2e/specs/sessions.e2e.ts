import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { App, createHome, removeHome } from "../src/app.ts";
import { chord } from "../src/keys.ts";
import { inFront, stepTimer } from "../src/sessions.ts";
import { Sidebar } from "../src/sidebar.ts";

/** The shortcut that splits the focused pane side by side. */
const SPLIT_RIGHT =
  process.platform === "darwin"
    ? chord({ key: "d", code: "KeyD", keyCode: 68 }, { meta: true })
    : chord({ key: "D", code: "KeyD", keyCode: 68 }, { ctrl: true, shift: true });

describe("several sessions", () => {
  let home: string | undefined;
  const apps: App[] = [];

  beforeAll(async () => {
    home = await createHome();
  });

  afterAll(async () => {
    for (const app of apps) await app.dispose();
    if (home !== undefined) await removeHome(home);
  }, 120_000);

  it("are listed, renamed, split, switched and restored dormant", async () => {
    if (home === undefined) throw new Error("no home directory");
    const step = stepTimer("sessions");
    const first = await App.launch("sessions-first", { home });
    step("launch");
    apps.push(first);
    const sidebar = new Sidebar(first.driver);
    const { terminal } = first;
    const [original] = await sidebar.waitForRows("the first session", (rows) => rows.length === 1);
    if (!original) throw new Error("no first session");

    // A second session, a shell in the most recent folder, from the dialog.
    await sidebar.click('[data-testid="new-session-button"]');
    await sidebar.clickWhenThere('[data-testid="kind-shell"]');
    await sidebar.click('[data-testid="create-session"]');
    const two = await sidebar.waitForRows(
      "the second session in the sidebar",
      (rows) => rows.length === 2 && rows.some((row) => row.active && row.id !== original.id),
    );
    const second = two.find((row) => row.id !== original.id);
    if (!second) throw new Error("no second session");
    expect(second.title).not.toBe("");
    const started = await inFront(
      terminal,
      "the second session to run in the focused pane",
      (state) => state.session === second.id && state.status.kind === "running",
    );
    await terminal.waitForPrompt();
    step("second session at its prompt");

    // Renamed: the name is what the sidebar shows from then on.
    await sidebar.rename(second.id, "Second shell");
    await sidebar.waitForRows("the new name", (rows) =>
      rows.some((row) => row.id === second.id && row.title === "Second shell"),
    );

    step("rename");

    // Split with the shortcut: a new, empty pane takes the focus, and the
    // first session goes in it.
    await terminal.keys(SPLIT_RIGHT);
    await sidebar.waitForPanes(
      "an empty pane next to the first",
      (panes) => panes.length === 2 && panes.some((pane) => pane.focused && pane.title === null),
    );
    await sidebar.click('[data-testid="empty-pane"] .link-button');
    await sidebar.waitForPanes(
      "both sessions on screen",
      (panes) => panes.length === 2 && panes.every((pane) => pane.title !== null),
    );
    // The first session kept running while it was off the screen.
    await inFront(
      terminal,
      "the first session to be in front, running",
      (state) => state.session === original.id && state.status.kind === "running",
    );

    // Switching back puts the second session in front, still running in
    // the same terminal: its process was not started again.
    await sidebar.clickRow(second.id);
    const switched = await inFront(
      terminal,
      "the second session to be in front",
      (state) => state.session === second.id,
    );
    expect(switched.status).toEqual(started.status);

    step("split and switch");
    expect(await first.quit()).toEqual({ code: 0, signal: null });
    step("quit");

    // Started again: both sessions are back, with their name, not running.
    const again = await App.launch("sessions-second", { home, restored: true });
    step("launch again");
    apps.push(again);
    const restoredSidebar = new Sidebar(again.driver);
    const restored = await restoredSidebar.waitForRows(
      "both sessions to be restored",
      (rows) => rows.length === 2,
    );
    expect(restored.map((row) => row.status)).toEqual(["dormant", "dormant"]);
    expect(restored.find((row) => row.id === second.id)?.title).toBe("Second shell");
    await restoredSidebar.waitForPanes("the two panes", (panes) => panes.length === 2);
    expect(await restoredSidebar.has('[data-testid="restore-banner"]')).toBe(true);
    step("restored");
  });
});
