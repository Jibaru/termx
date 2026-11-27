// Builds the application for the suite: `pnpm e2e:build`, after the
// frontend.
//
// The `e2e` feature embeds a WebDriver server, so the application refuses
// to compile it unless TERMX_ALLOW_E2E=1 (see apps/desktop/src-tauri/build.rs).
// This script and the End-to-end workflow are the only places that set it.
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

// The hook sender first: the application finds it next to itself.
const sender = spawnSync("cargo", ["build", "-p", "termx-cli", "--locked"], {
  cwd: fileURLToPath(new URL("..", import.meta.url)),
  stdio: "inherit",
});
if (sender.error) throw sender.error;
if (sender.status !== 0) process.exit(sender.status ?? 1);

const result = spawnSync(
  "cargo",
  ["build", "-p", "termx-desktop", "--features", "e2e,tauri/custom-protocol", "--locked"],
  {
    cwd: fileURLToPath(new URL("..", import.meta.url)),
    env: { ...process.env, TERMX_ALLOW_E2E: "1" },
    stdio: "inherit",
  },
);

if (result.error) throw result.error;
process.exit(result.status ?? 1);
