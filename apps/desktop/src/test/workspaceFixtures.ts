import type { Layout, Session, SessionId, WorkspaceSnapshot } from "../lib/ipc";

/**
 * Sessions and workspaces for tests, shaped as the Rust side sends them.
 * `ipc.contract.test.ts` keeps the shapes honest.
 */

/** A shell session in `/work/app`, with whatever differs. */
export function session(id: SessionId, overrides: Partial<Session> = {}): Session {
  return {
    id,
    kind: "shell",
    cwd: "/work/app",
    liveCwd: null,
    command: null,
    title: "Shell",
    name: null,
    autoTitle: null,
    project: { key: "/work/app", name: "app", root: "/work/app", kind: "git" },
    git: { branch: "main", commit: null, worktree: null },
    state: { status: "starting" },
    terminalId: null,
    createdAt: 1_000,
    lastActivityAt: 1_000,
    waitingSince: null,
    claudeSessionId: null,
    warning: null,
    ...overrides,
  };
}

/** One pane, showing `shown` or nothing. */
export function singlePane(shown: SessionId | null = null): Layout {
  return { root: { type: "pane", id: 1, session: shown }, focused: 1, nextId: 2 };
}

/** A workspace with these sessions, the first of them in its only pane. */
export function workspace(
  sessions: Session[] = [],
  overrides: Partial<WorkspaceSnapshot> = {},
): WorkspaceSnapshot {
  const projects = [...new Map(sessions.map((s) => [s.project.key, s.project])).values()];
  return {
    revision: sessions.length,
    sessions,
    projects,
    layout: singlePane(sessions[0]?.id ?? null),
    recentFolders: [],
    persistence: { mode: "primary" },
    ...overrides,
  };
}
