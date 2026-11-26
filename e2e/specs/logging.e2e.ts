import { describe, expect, it } from "vitest";

import { useApp } from "../src/fixture.ts";
import { waitFor } from "../src/wait.ts";

describe("the log file", () => {
  const fixture = useApp("logging");

  it("records an uncaught error of the frontend, and nothing of the terminal", async () => {
    const { app, terminal } = fixture;
    // Something typed and printed, which must never reach the log, even at
    // the debug level the suite runs the application with.
    const secret = `secret-${terminal.token()}`;
    await terminal.submit(`echo ${secret}`);
    await terminal.waitForExactLine(secret);

    const message = `forced by the end-to-end suite ${terminal.token()}`;
    await terminal.throwUncaught(message);

    const logs = await waitFor(
      async () => {
        const text = await app.readLogs();
        return text.includes(message) ? text : undefined;
      },
      { what: "the error to be written to the log file" },
    );
    const record = logs.split("\n").find((line) => line.includes(message)) ?? "";
    expect(record).toContain("ERROR");
    expect(record).toContain("window.onerror");
    expect(logs).not.toContain(secret);

    // Still nothing once everything is flushed, at exit.
    await app.quit();
    const flushed = await app.readLogs();
    expect(flushed).toContain(message);
    expect(flushed).toContain("termx exiting");
    expect(flushed).not.toContain(secret);
  });
});
