import { describe, expect, it } from "vitest";

import { LogLimiter, type LogLimits } from "./logLimiter";

const LIMITS: LogLimits = {
  perLevel: { error: 3, warn: 2, info: 2 },
  newErrors: 2,
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

    const infos = Array.from({ length: 100 }, () => limit.admit("info", "noise"));
    const warnings = Array.from({ length: 100 }, () => limit.admit("warn", "noise"));

    expect(infos.filter((result) => result !== null)).toHaveLength(2);
    expect(warnings.filter((result) => result !== null)).toHaveLength(2);
    // The noise did not touch the room for errors.
    expect(limit.admit("error", "real problem")).not.toBeNull();
  });

  it("lets the first occurrence of an error through a flood of repeats", () => {
    const { limiter: limit } = limiter();

    const repeats = Array.from({ length: 100 }, () => limit.admit("error", "render loop"));
    expect(repeats.filter((result) => result !== null)).toHaveLength(3);

    expect(limit.admit("error", "disk full")).toEqual({ error: 97, warn: 0, info: 0 });
    expect(limit.admit("error", "disk full")).toBeNull();
  });

  it("bounds distinct errors too", () => {
    const { limiter: limit } = limiter();

    const results = Array.from({ length: 1_000 }, (_, index) =>
      limit.admit("error", `error ${String(index)}`),
    );

    expect(results.filter((result) => result !== null)).toHaveLength(5);
  });

  it("counts what it drops per level and hands the counts out once", () => {
    const { clock, limiter: limit } = limiter();
    for (let index = 0; index < 10; index += 1) {
      limit.admit("error", "e");
      limit.admit("warn", "w");
      limit.admit("info", "i");
    }

    clock.now = 1_000;
    expect(limit.admit("info", "i")).toEqual({ error: 7, warn: 8, info: 8 });
    expect(limit.admit("error", "e")).toEqual(NOTHING);
  });

  it("starts over in the next window", () => {
    const { clock, limiter: limit } = limiter();
    for (let index = 0; index < 10; index += 1) limit.admit("warn", "w");
    expect(limit.admit("warn", "w")).toBeNull();

    clock.now = 999;
    expect(limit.admit("warn", "w")).toBeNull();
    clock.now = 1_000;
    expect(limit.admit("warn", "w")).not.toBeNull();
  });
});
