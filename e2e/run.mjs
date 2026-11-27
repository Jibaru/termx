// Runs the suite: `pnpm e2e [arguments of vitest]`.
//
// On Windows the suite is first given back the default handling of Ctrl+C.
// A process can be started with Ctrl+C ignored, and passes that on to
// everything it starts: CI runners start their steps that way. The
// application under test would pass it on to the shells of its terminals,
// where Ctrl+C would then interrupt nothing. A desktop starts applications
// with Ctrl+C handled, so that is what the suite starts from; the test in
// `specs/ctrl-c.e2e.ts` covers the other case on purpose.
import { spawnSync } from "node:child_process";
import { rmSync } from "node:fs";
import { fileURLToPath } from "node:url";

// The starts repeated in this run, and only those (see src/app.ts).
rmSync(fileURLToPath(new URL("./artifacts/startup-retries.log", import.meta.url)), {
  force: true,
});

const vitest = [fileURLToPath(new URL("./node_modules/vitest/vitest.mjs", import.meta.url)), "run"];
const args = process.argv.slice(2);

const result =
  process.platform === "win32"
    ? spawnSync(
        "powershell.exe",
        [
          "-NoProfile",
          "-NonInteractive",
          "-ExecutionPolicy",
          "Bypass",
          "-File",
          fileURLToPath(new URL("./restore-ctrl-c.ps1", import.meta.url)),
          process.execPath,
          ...vitest,
          ...args,
        ],
        { stdio: "inherit" },
      )
    : spawnSync(process.execPath, [...vitest, ...args], { stdio: "inherit" });

if (result.error) throw result.error;
process.exit(result.status ?? 1);
