import { invoke } from "@tauri-apps/api/core";

/**
 * Typed wrappers around the Tauri commands exposed by the Rust side
 * (apps/desktop/src-tauri/src/commands). Components call these instead of
 * using `invoke` directly, so command names live in a single place.
 */

/** Returns the version of the `termx-core` library the app was built with. */
export function getCoreVersion(): Promise<string> {
  return invoke<string>("core_version");
}
