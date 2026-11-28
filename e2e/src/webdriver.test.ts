import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";

import { afterEach, describe, expect, it } from "vitest";

import { WebDriver } from "./webdriver.ts";

/**
 * A WebDriver server that behaves as the one of the application on macOS
 * while its page reloads: the script that reaches the page as it goes away
 * is never answered, and the server only says so once the script timeout
 * of the session is over.
 */
class ReloadingServer {
  /** The script timeout of the session, as the client last set it. */
  scriptTimeoutMs = 30_000;
  /** Scripts run so far. */
  scripts = 0;
  private readonly server: Server;

  constructor(
    /** Which script, counting from 1, is lost with the old page. */
    private readonly lost: number,
    /** From which script on the new page is there. */
    private readonly loadedFrom: number,
  ) {
    this.server = createServer((request, response) => {
      let body = "";
      request.on("data", (chunk: Buffer) => (body += chunk.toString()));
      request.on("end", () => {
        this.answer(request.url ?? "", body, (status, value) => {
          response.writeHead(status, { "content-type": "application/json" });
          response.end(JSON.stringify({ value }));
        });
      });
    });
  }

  private answer(url: string, body: string, send: (status: number, value: unknown) => void) {
    if (url === "/session") {
      send(200, { sessionId: "s" });
    } else if (url === "/session/s/timeouts") {
      this.scriptTimeoutMs = (JSON.parse(body) as { script: number }).script;
      send(200, null);
    } else if (url === "/session/s/execute/sync") {
      this.scripts += 1;
      if (this.scripts === this.lost) {
        setTimeout(() => {
          send(500, { error: "script timeout", message: "" });
        }, this.scriptTimeoutMs);
      } else {
        send(200, this.scripts >= this.loadedFrom);
      }
    } else {
      send(404, { error: "unknown command", message: url });
    }
  }

  async start(): Promise<string> {
    await new Promise<void>((resolve) => this.server.listen(0, "127.0.0.1", resolve));
    return `http://127.0.0.1:${String((this.server.address() as AddressInfo).port)}`;
  }

  stop(): Promise<void> {
    this.server.closeAllConnections();
    return new Promise((resolve) =>
      this.server.close(() => {
        resolve();
      }),
    );
  }
}

describe("waiting for the page that replaces another", () => {
  let server: ReloadingServer | undefined;

  afterEach(async () => {
    await server?.stop();
    server = undefined;
  });

  it("gives up quickly on a question the old page never answers", async () => {
    server = new ReloadingServer(2, 3);
    const driver = await WebDriver.connect(await server.start());
    // The timeout the suite runs with, as after the start of the application.
    server.scriptTimeoutMs = 15_000;

    const started = performance.now();
    await driver.waitForPage(30_000, "return !window.__OLD__;");
    const elapsed = performance.now() - started;

    // One lost question costs the short timeout of the probe, not 15 s.
    expect(server.scripts).toBe(3);
    expect(elapsed).toBeLessThan(3_000);
    expect(server.scriptTimeoutMs).toBe(15_000);
  });

  it("gives the scripts their usual timeout back when the page never comes", async () => {
    server = new ReloadingServer(0, Number.POSITIVE_INFINITY);
    const driver = await WebDriver.connect(await server.start());

    await expect(driver.waitForPage(300, "return false;")).rejects.toThrow(/Timed out/);
    expect(server.scriptTimeoutMs).toBe(15_000);
  });
});
