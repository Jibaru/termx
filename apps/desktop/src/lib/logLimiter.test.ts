import { describe, expect, it } from "vitest";

import { LogLimiter, type LogLimits, messageKey } from "./logLimiter";

const LIMITS: LogLimits = {
  perLevel: { error: 3, warn: 2, info: 2 },
  newErrorsPerSource: 2,
  windowMs: 1_000,
};

function limiter() {
  const clock = { now: 0 };
  return { clock, limiter: new LogLimiter(LIMITS, () => clock.now) };
}

const NOTHING = { error: 0, warn: 0, info: 0 };

describe("LogLimiter", () => {
  it("gives every level its own budget", () => {
    const { limiter: limit } = limiter();

    const infos = Array.from({ length: 100 }, () => limit.admit("info", "ui\nnoise"));
    const warnings = Array.from({ length: 100 }, () => limit.admit("warn", "ui\nnoise"));

    expect(infos.filter((result) => result !== null)).toHaveLength(2);
    expect(warnings.filter((result) => result !== null)).toHaveLength(2);
    // The noise did not touch the room for errors.
    expect(limit.admit("error", "ui\nreal problem")).not.toBeNull();
  });

  it("lets the first occurrence of an error through a flood of repeats", () => {
    const { limiter: limit } = limiter();

    const repeats = Array.from({ length: 100 }, () => limit.admit("error", "ui\nrender loop"));
    expect(repeats.filter((result) => result !== null)).toHaveLength(3);

    expect(limit.admit("error", "ui\ndisk full")).toEqual({ error: 97, warn: 0, info: 0 });
    expect(limit.admit("error", "ui\ndisk full")).toBeNull();
  });

  it("treats numbered variants of one error as repeats", () => {
    const { limiter: limit } = limiter();

    const results = Array.from({ length: 10_000 }, (_, index) =>
      limit.admit("error", `loop\nrequest ${String(index)} failed for 7f3a${index.toString(16)}`),
    );
    expect(results.filter((result) => result !== null)).toHaveLength(3);

    // The budget for new errors is untouched, even for the same source.
    expect(limit.admit("error", "loop\nthe disk is full")).not.toBeNull();
  });

  it("lets a flooding source exhaust only its own budget for new errors", () => {
    const { limiter: limit } = limiter();
    const unique = (index: number) =>
      `failure ${Array.from(String(index), (digit) => "abcdefghij"[Number(digit)]).join("")}`;

    const results = Array.from({ length: 10_000 }, (_, index) =>
      limit.admit("error", `loop\n${unique(index)}`),
    );
    expect(results.filter((result) => result !== null)).toHaveLength(5);
    expect(limit.admit("error", "loop\none more")).toBeNull();

    // Another source has its own.
    expect(limit.admit("error", "disk\nthe disk is full")).not.toBeNull();
  });

  it("counts what it drops per level and hands the counts out once", () => {
    const { clock, limiter: limit } = limiter();
    for (let index = 0; index < 10; index += 1) {
      limit.admit("error", "ui\ne");
      limit.admit("warn", "ui\nw");
      limit.admit("info", "ui\ni");
    }

    clock.now = 1_000;
    expect(limit.admit("info", "ui\ni")).toEqual({ error: 7, warn: 8, info: 8 });
    expect(limit.admit("error", "ui\ne")).toEqual(NOTHING);
  });

  it("starts over in the next window", () => {
    const { clock, limiter: limit } = limiter();
    for (let index = 0; index < 10; index += 1) limit.admit("warn", "ui\nw");
    expect(limit.admit("warn", "ui\nw")).toBeNull();

    clock.now = 999;
    expect(limit.admit("warn", "ui\nw")).toBeNull();
    clock.now = 1_000;
    expect(limit.admit("warn", "ui\nw")).not.toBeNull();
  });
});

describe("messageKey", () => {
  it("ignores numbers, identifiers, case and white space", () => {
    expect(messageKey("Error 1")).toBe("error #");
    expect(messageKey("session 7f3a9c2e closed after 1500ms")).toBe(
      messageKey("session 00ab12cd  closed after 3ms"),
    );
    expect(messageKey("request 123e4567-e89b-12d3-a456-426614174000 failed")).toBe(
      "request # failed",
    );
    expect(messageKey("cannot read 0x7ffee4b2 at 10.0.0.12:8080")).toBe(
      messageKey("cannot read 0x1000 at 192.168.1.1:443"),
    );
  });

  it("keeps different problems apart", () => {
    expect(messageKey("disk full")).not.toBe(messageKey("render loop"));
    expect(messageKey("exit code 3: not found")).toBe("exit code #: not found");
  });
});
