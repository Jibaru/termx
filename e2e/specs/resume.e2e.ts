import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { App, createHome, removeHome } from "../src/app.ts";
import { inFront, newShell, stepTimer } from "../src/sessions.ts";
import { Sidebar } from "../src/sidebar.ts";

describe("resuming restored sessions", () => {
  let home: string | undefined;
  const apps: App[] = [];

  beforeAll(async () => {
    home = await createHome();
  });

  afterAll(async () => {
    for (const app of apps) await app.dispose();
    if (home !== undefined) await removeHome(home);
  }, 120_000);

  it("resumes one from its pane, then the rest from the banner", async () => {
    if (home === undefined) throw new Error("no home directory");
    const step = stepTimer("resume");
    const first = await App.launch("resume-first", { home });
    apps.push(first);
    const firstSidebar = new Sidebar(first.driver);
    const original = (await first.terminal.state()).session;
    const other = await newShell(firstSidebar, first.terminal);
    // The first one in front again, so that it is the one restored in front.
    await firstSidebar.clickRow(original);
    await inFront(
      first.terminal,
      "the first session in front",
      (state) => state.session === original,
    );
    expect(await first.quit()).toEqual({ code: 0, signal: null });
    step("two sessions, then quit");

    const second = await App.launch("resume-second", { home, restored: true });
    apps.push(second);
    const sidebar = new Sidebar(second.driver);
    const { terminal } = second;
    await sidebar.waitForRows(
      "both sessions, not running",
      (rows) => rows.length === 2 && rows.every((row) => row.status === "dormant"),
    );
    expect(await sidebar.has('[data-testid="restore-banner"]')).toBe(true);
    step("restored");

    // Resume in the pane: only that one starts.
    await sidebar.click('[data-testid="pane-resume"]');
    await inFront(
      terminal,
      "the session in front to run again",
      (state) => state.session === original && state.status.kind === "running",
    );
    await terminal.waitForPrompt();
    const rows = await sidebar.waitForRows("one running, one not", (list) =>
      list.some((row) => row.id === original && row.status === "running"),
    );
    expect(rows.find((row) => row.id === other)?.status).toBe("dormant");
    step("resumed from the pane");

    // Resume all: the one off the screen starts too, without being shown.
    await sidebar.clickWhenThere('[data-testid="resume-all"]');
    await sidebar.waitForRows("both running", (list) =>
      list.every((row) => row.status === "running"),
    );
    expect((await terminal.state()).session).toBe(original);
    expect(await sidebar.has('[data-testid="restore-banner"]')).toBe(false);
    step("resumed all");
  });
});
