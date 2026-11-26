import "@testing-library/jest-dom/vitest";

import { clearMocks } from "@tauri-apps/api/mocks";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

import { resetFrontendLogLimit } from "../lib/ipc";

afterEach(() => {
  cleanup();
  clearMocks();
  resetFrontendLogLimit();
});
