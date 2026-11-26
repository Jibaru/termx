import { describe, it } from "vitest";

import { useApp } from "../src/fixture.ts";
import { enter } from "../src/keys.ts";

describe("typing in the terminal", () => {
  const fixture = useApp("typing");

  it("runs a typed command and shows its output", async () => {
    const { terminal } = fixture;
    // Lower and upper case, digits and punctuation: xterm.js takes some of
    // these from the key going down and others from the character typed.
    // None of them means anything to bash or to cmd.exe.
    const text = `Hello-World_42+x=y/z,[a]{b}~@#:${terminal.token()}`;

    await terminal.type(`echo ${text}`);
    await terminal.waitForLine("the typed command line", (line) => line.endsWith(`echo ${text}`));
    await terminal.keys(enter());

    await terminal.waitForExactLine(text);
  });

  it("keeps the keys of a fast typist in order", async () => {
    const { terminal } = fixture;
    const words = Array.from({ length: 40 }, (_, index) => `w${String(index)}`).join("-");
    const text = `${words}-${terminal.token()}`;

    await terminal.submit(`echo ${text}`);

    await terminal.waitForExactLine(text);
  });
});
