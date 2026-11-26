import { useEffect, useState } from "react";

import { getCoreVersion } from "./ipc";

export type CoreVersionState =
  { status: "loading" } | { status: "ready"; version: string } | { status: "error" };

/** Fetches the core library version from the Rust side once on mount. */
export function useCoreVersion(): CoreVersionState {
  const [state, setState] = useState<CoreVersionState>({ status: "loading" });

  useEffect(() => {
    let cancelled = false;

    getCoreVersion()
      .then((version) => {
        if (!cancelled) setState({ status: "ready", version });
      })
      .catch(() => {
        if (!cancelled) setState({ status: "error" });
      });

    return () => {
      cancelled = true;
    };
  }, []);

  return state;
}
