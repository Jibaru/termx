import { waitFor } from "./wait.ts";
import type { WebDriver } from "./webdriver.ts";

/**
 * The sidebar and the panes of the window, read and driven through their
 * `data-testid` hooks with small synchronous scripts.
 *
 * Clicks are delivered as DOM events, as the suite delivers keys: it never
 * moves the pointer of the machine it runs on.
 */

export interface SidebarRow {
  id: string;
  title: string;
  /** `dormant`, `starting`, `running` or `exited`. */
  status: string;
  /** Shown in the focused pane. */
  active: boolean;
}

export interface PaneInfo {
  id: string;
  focused: boolean;
  /** The title in the header of the pane, `null` for an empty pane. */
  title: string | null;
}

export class Sidebar {
  constructor(private readonly driver: WebDriver) {}

  rows(): Promise<SidebarRow[]> {
    return this.driver.execute<SidebarRow[]>(
      `return [...document.querySelectorAll('[data-testid="session-row"]')].map((row) => ({
         id: row.dataset.sessionId,
         title: row.querySelector(".row-title")?.textContent ?? "",
         status: row.dataset.status,
         active: row.getAttribute("aria-selected") === "true",
       }));`,
    );
  }

  panes(): Promise<PaneInfo[]> {
    return this.driver.execute<PaneInfo[]>(
      `return [...document.querySelectorAll('[data-testid="pane"]')].map((pane) => ({
         id: pane.dataset.paneId,
         focused: pane.dataset.focused === "true",
         title: pane.querySelector('[data-testid="empty-pane"]')
           ? null
           : pane.querySelector(".pane-title")?.textContent ?? null,
       }));`,
    );
  }

  /** Waits until the rows satisfy `check`, and returns them. */
  async waitForRows(what: string, check: (rows: SidebarRow[]) => boolean): Promise<SidebarRow[]> {
    let last: SidebarRow[] = [];
    await waitFor(
      async () => {
        last = await this.rows();
        return check(last);
      },
      { what, describe: () => last },
    );
    return last;
  }

  /** Waits until the panes satisfy `check`, and returns them. */
  async waitForPanes(what: string, check: (panes: PaneInfo[]) => boolean): Promise<PaneInfo[]> {
    let last: PaneInfo[] = [];
    await waitFor(
      async () => {
        last = await this.panes();
        return check(last);
      },
      { what, describe: () => last },
    );
    return last;
  }

  /** Clicks the first element that matches `selector`. */
  async click(selector: string): Promise<void> {
    const found = await this.driver.execute<boolean>(
      `const element = document.querySelector(arguments[0]);
       if (!element) return false;
       element.click();
       return true;`,
      selector,
    );
    if (!found) throw new Error(`nothing on the page matches ${selector}`);
  }

  /** Whether an element matches `selector`. */
  has(selector: string): Promise<boolean> {
    return this.driver.execute<boolean>(
      `return document.querySelector(arguments[0]) !== null;`,
      selector,
    );
  }

  /** Clicks once an element matches `selector`. */
  async clickWhenThere(selector: string): Promise<void> {
    await waitFor(() => this.has(selector), { what: `${selector} to be on the page` });
    await this.click(selector);
  }

  /** Clicks the row of a session, which puts it in front. */
  clickRow(id: string): Promise<void> {
    return this.click(`[data-testid="session-row"][data-session-id="${id}"]`);
  }

  /** Renames a session the way a user does: double-click, type, Enter. */
  async rename(id: string, name: string): Promise<void> {
    await this.driver.execute(
      `const row = document.querySelector(
         '[data-testid="session-row"][data-session-id="' + arguments[0] + '"]');
       row.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
       return true;`,
      id,
    );
    await waitFor(
      () =>
        this.driver.execute<boolean>(
          `const input = document.querySelector('[data-testid="rename-input"]');
           if (!input) return false;
           input.value = arguments[0];
           input.dispatchEvent(new KeyboardEvent("keydown", {
             key: "Enter", code: "Enter", bubbles: true, cancelable: true }));
           return true;`,
          name,
        ),
      { what: "the name of the session to be editable" },
    );
  }
}
