import { fireEvent, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { NO_CLAUDE } from "../claude/backend";
import type { ClaudeIntegration, CommandError } from "../lib/ipc";
import { fakeClaude, INSTALLED, PREVIEW } from "../test/fakeClaude";
import { flush, renderApp } from "../test/fakeWorkspace";
import { session, workspace } from "../test/workspaceFixtures";

/** Claude Code is there, and the user was never asked. */
const FIRST_LAUNCH: ClaudeIntegration = {
  claudeInstalled: true,
  plugin: "notInstalled",
  consent: "unasked",
};

const shell = () => workspace([session("s-1", { state: { status: "running" }, terminalId: 1 })]);

function failure(kind: string, message: string): CommandError {
  return { command: "install_claude_integration", kind, message } as CommandError;
}

afterEach(() => {
  window.localStorage.clear();
});

async function openSetupFromBanner() {
  fireEvent.click(screen.getByTestId("claude-consent-open"));
  await flush();
  return screen.getByTestId("claude-setup-dialog");
}

describe("the first launch with Claude Code", () => {
  it("asks with a banner, and explains exactly what installing changes", async () => {
    const fake = fakeClaude(FIRST_LAUNCH);
    await renderApp(shell(), { claude: fake });

    expect(screen.getByTestId("claude-consent-banner")).toHaveTextContent(
      "See which Claude session needs you",
    );
    // Not modal: nothing else is in the way.
    expect(screen.queryByRole("dialog")).toBeNull();

    const dialog = await openSetupFromBanner();
    expect(dialog).toHaveAccessibleName("Show Claude Code status in termx");
    expect(dialog).toHaveTextContent(
      "termx shows when each Claude session is working or needs you",
    );
    const changes = within(dialog).getByTestId("claude-changes");
    for (const change of PREVIEW.changes) {
      expect(changes).toHaveTextContent(change.path);
      expect(changes).toHaveTextContent(change.description);
    }
    expect(dialog).toHaveTextContent(
      "Your existing Claude Code hooks and settings are not changed. You can remove the integration at any time",
    );
    expect(document.activeElement).toBe(within(dialog).getByTestId("claude-install"));
    expect(screen.queryByTestId("claude-consent-banner")).toBeNull();
  });

  it("records the consent, then installs, and says it is installed", async () => {
    const fake = fakeClaude(FIRST_LAUNCH);
    await renderApp(shell(), { claude: fake });

    const dialog = await openSetupFromBanner();
    fireEvent.click(within(dialog).getByTestId("claude-install"));
    await flush();

    const order = fake.calls
      .map((call) => call.command)
      .filter(
        (command) => command === "setClaudeConsent" || command === "installClaudeIntegration",
      );
    expect(order).toEqual(["setClaudeConsent", "installClaudeIntegration"]);
    expect(fake.callsOf("setClaudeConsent")).toEqual([["accepted"]]);
    expect(screen.queryByTestId("claude-setup-dialog")).toBeNull();
    expect(screen.getByTestId("claude-status")).toHaveTextContent(
      "Claude Code integration: installed v1.1.0 · Remove",
    );
    expect(screen.getByTestId("hint")).toHaveTextContent("Claude Code integration installed");
    expect(screen.queryByTestId("claude-consent-banner")).toBeNull();
  });

  it("shows progress while it installs", async () => {
    let finish: () => void = () => undefined;
    const fake = fakeClaude(FIRST_LAUNCH, {
      installClaudeIntegration: () =>
        new Promise<void>((resolve) => {
          finish = resolve;
        }),
    });
    await renderApp(shell(), { claude: fake });

    const dialog = await openSetupFromBanner();
    fireEvent.click(within(dialog).getByTestId("claude-install"));
    await flush();

    expect(within(dialog).getByTestId("claude-installing")).toHaveTextContent("Installing…");
    expect(within(dialog).getByTestId("claude-install")).toBeDisabled();
    expect(within(dialog).getByTestId("claude-not-now")).toBeDisabled();
    finish();
    await flush();
    expect(screen.queryByTestId("claude-setup-dialog")).toBeNull();
  });

  it.each([
    [
      "permissionDenied",
      "~/.claude is read-only",
      "Install failed. termx is not allowed to write to the folder of Claude Code: ~/.claude is read-only",
    ],
    ["storageFull", "", "Install failed. The disk is full."],
    ["somethingNew", "The plugin folder is a file", "Install failed. The plugin folder is a file"],
  ])("says why an install failed (%s), and can try again", async (kind, message, expected) => {
    let attempts = 0;
    const fake = fakeClaude(FIRST_LAUNCH, {
      installClaudeIntegration: () => {
        attempts += 1;
        // eslint-disable-next-line @typescript-eslint/only-throw-error -- Tauri rejects with the serialised error.
        if (attempts === 1) throw failure(kind, message);
      },
    });
    await renderApp(shell(), { claude: fake });

    const dialog = await openSetupFromBanner();
    fireEvent.click(within(dialog).getByTestId("claude-install"));
    await flush();

    expect(within(dialog).getByRole("alert")).toHaveTextContent(expected);
    const retry = within(dialog).getByTestId("claude-install");
    expect(retry).toHaveTextContent("Try again");
    fireEvent.click(retry);
    await flush();
    expect(screen.queryByTestId("claude-setup-dialog")).toBeNull();
    expect(fake.callsOf("installClaudeIntegration")).toHaveLength(2);
  });

  it("says when it cannot tell what would change", async () => {
    const fake = fakeClaude(FIRST_LAUNCH, {
      previewClaudeIntegration: () => {
        // eslint-disable-next-line @typescript-eslint/only-throw-error -- Tauri rejects with the serialised error.
        throw failure("internal", "settings.json is not valid JSON");
      },
    });
    await renderApp(shell(), { claude: fake });

    const dialog = await openSetupFromBanner();

    expect(within(dialog).getByRole("alert")).toHaveTextContent(
      "termx could not tell what would change: settings.json is not valid JSON",
    );
  });
});

describe("not now", () => {
  it("from the banner, stops asking and leaves a way back in the sidebar", async () => {
    const fake = fakeClaude(FIRST_LAUNCH);
    await renderApp(shell(), { claude: fake });

    fireEvent.click(screen.getByTestId("claude-consent-not-now"));
    await flush();

    expect(fake.callsOf("setClaudeConsent")).toEqual([["declined"]]);
    expect(fake.callsOf("installClaudeIntegration")).toEqual([]);
    expect(screen.queryByTestId("claude-consent-banner")).toBeNull();
    const enable = screen.getByTestId("claude-enable");
    expect(enable).toHaveTextContent("Enable Claude Code status");

    enable.focus();
    fireEvent.click(enable);
    await flush();
    const dialog = screen.getByTestId("claude-setup-dialog");
    // Escape closes it, and the focus goes back to where it was.
    fireEvent.keyDown(dialog, { key: "Escape" });
    await flush();
    expect(screen.queryByTestId("claude-setup-dialog")).toBeNull();
    expect(document.activeElement).toBe(screen.getByTestId("claude-enable"));
  });

  it("from the dialog, records the answer and closes it", async () => {
    const fake = fakeClaude(FIRST_LAUNCH);
    await renderApp(shell(), { claude: fake });

    const dialog = await openSetupFromBanner();
    fireEvent.click(within(dialog).getByTestId("claude-not-now"));
    await flush();

    expect(screen.queryByTestId("claude-setup-dialog")).toBeNull();
    expect(fake.callsOf("setClaudeConsent")).toEqual([["declined"]]);
    expect(screen.getByTestId("claude-enable")).toBeInTheDocument();
  });

  it("is remembered: a declined integration does not ask again", async () => {
    await renderApp(shell(), { claude: fakeClaude({ ...FIRST_LAUNCH, consent: "declined" }) });

    expect(screen.queryByTestId("claude-consent-banner")).toBeNull();
    expect(screen.getByTestId("claude-enable")).toBeInTheDocument();
  });
});

describe("the integration in the sidebar footer", () => {
  it("offers an update when the plugin is outdated", async () => {
    const fake = fakeClaude({ ...INSTALLED, plugin: "outdated", version: "0.9.0" });
    await renderApp(shell(), { claude: fake });

    expect(screen.getByTestId("claude-status")).toHaveTextContent(
      "Claude Code integration: an update is available (installed v0.9.0)",
    );
    fireEvent.click(screen.getByTestId("claude-update"));
    await flush();
    const dialog = screen.getByTestId("claude-setup-dialog");
    expect(dialog).toHaveAccessibleName("Update the Claude Code integration");
    fireEvent.click(within(dialog).getByTestId("claude-install"));
    await flush();

    expect(fake.callsOf("installClaudeIntegration")).toHaveLength(1);
    // Consent was given already.
    expect(fake.callsOf("setClaudeConsent")).toEqual([]);
    expect(screen.getByTestId("claude-status")).toHaveTextContent("installed v1.1.0");
  });

  it("removes the integration after asking", async () => {
    const fake = fakeClaude();
    await renderApp(shell(), { claude: fake });

    fireEvent.click(screen.getByTestId("claude-remove"));
    await flush();
    const dialog = screen.getByTestId("claude-remove-dialog");
    expect(dialog).toHaveTextContent("Your own hooks and settings are not touched.");
    fireEvent.click(within(dialog).getByTestId("claude-remove-confirm"));
    await flush();

    expect(fake.callsOf("uninstallClaudeIntegration")).toHaveLength(1);
    expect(fake.callsOf("setClaudeConsent")).toEqual([["declined"]]);
    expect(screen.queryByTestId("claude-remove-dialog")).toBeNull();
    expect(screen.getByTestId("claude-enable")).toBeInTheDocument();
  });

  it("offers a repair when the plugin is broken", async () => {
    await renderApp(shell(), {
      claude: fakeClaude({
        ...INSTALLED,
        plugin: "error",
        error: { kind: "notFound", message: "hooks.json is missing" },
      }),
    });

    expect(screen.getByTestId("claude-status")).toHaveTextContent(
      "Claude Code integration has a problem: A file termx needs was not found: hooks.json is missing",
    );
    expect(screen.getByTestId("claude-repair")).toBeInTheDocument();
  });

  it("does not ask without Claude Code, and explains why it is off", async () => {
    const fake = fakeClaude({ claudeInstalled: false, plugin: "notInstalled", consent: "unasked" });
    await renderApp(shell(), { claude: fake });

    expect(screen.queryByTestId("claude-consent-banner")).toBeNull();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByTestId("claude-status")).toHaveTextContent(
      "Claude Code was not found on this computer",
    );

    fake.setState({ claudeInstalled: true });
    fireEvent.click(screen.getByRole("button", { name: "Check again" }));
    await flush();
    expect(screen.getByTestId("claude-consent-banner")).toBeInTheDocument();
  });

  it("shows nothing when this build has no integration", async () => {
    await renderApp(shell(), {
      claude: { ...fakeClaude(), backend: NO_CLAUDE },
    });

    expect(screen.queryByTestId("claude-status")).toBeNull();
    expect(screen.queryByTestId("claude-consent-banner")).toBeNull();
  });
});
