import { describe, expect, it } from "vitest";

import { describeForLog, describeIpcFailure, describeShape } from "./logSafe";

const SECRET = "hunter2-very-secret";

describe("describeForLog", () => {
  it("keeps the name, message and stack of an error", () => {
    const error = new TypeError("x is not a function");

    expect(describeForLog(error)).toEqual({
      message: "TypeError: x is not a function",
      stack: error.stack,
    });
    expect(describeForLog(new RangeError("")).message).toBe("RangeError");
  });

  it("leaves out the other properties of an error", () => {
    const error = Object.assign(new Error("request failed"), {
      response: { body: SECRET },
      token: SECRET,
    });

    expect(JSON.stringify(describeForLog(error))).not.toContain(SECRET);
  });

  it("logs only the type and the top-level keys of an object", () => {
    const described = describeForLog({ token: SECRET, env: { HOME: SECRET, API_KEY: SECRET } });

    expect(described).toEqual({ message: "non-Error value (object with keys [token, env])" });
    expect(JSON.stringify(described)).not.toContain(SECRET);
    expect(JSON.stringify(described)).not.toContain("API_KEY");
  });

  it("never logs the value of a string, a number or an array", () => {
    expect(describeForLog(SECRET).message).toBe(
      `non-Error value (string of length ${String(SECRET.length)})`,
    );
    expect(describeForLog(424242).message).toBe("non-Error value (number)");
    expect(describeForLog([SECRET, SECRET]).message).toBe("non-Error value (array of length 2)");
    expect(describeForLog(undefined).message).toBe("non-Error value (undefined)");
    expect(describeForLog(null).message).toBe("non-Error value (null)");
    expect(describeForLog(() => SECRET).message).toBe("non-Error value (function)");
  });

  it("caps the number and the length of the keys", () => {
    const wide = Object.fromEntries(
      Array.from({ length: 30 }, (_, index) => [`key${String(index)}`, 1]),
    );
    expect(describeShape(wide)).toContain("key19, and 10 more]");
    expect(describeShape({ ["k".repeat(100)]: 1 })).toBe(`object with keys [${"k".repeat(40)}]`);
  });
});

describe("describeIpcFailure", () => {
  it("classifies the string Tauri rejects with instead of copying it", () => {
    expect(describeIpcFailure("Command rename_session not found").message).toBe(
      "command not found (string of length 32)",
    );
    expect(describeIpcFailure("open_log_folder not allowed. Permissions ...").message).toMatch(
      /^not allowed \(/,
    );
    const invalid = describeIpcFailure(
      `invalid args \`data\` for command \`write\`: invalid type: string "${SECRET}", expected u32`,
    );
    expect(invalid.message).toMatch(/^invalid arguments \(string of length \d+\)$/);
    expect(invalid.message).not.toContain(SECRET);
    expect(describeIpcFailure(SECRET).message).toMatch(/^unrecognised failure \(/);
  });

  it("treats errors and other values like describeForLog", () => {
    expect(describeIpcFailure(new Error("boom")).message).toBe("Error: boom");
    expect(describeIpcFailure({ token: SECRET }).message).toBe(
      "non-Error value (object with keys [token])",
    );
  });
});
