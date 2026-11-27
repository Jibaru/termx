import { describe, expect, it } from "vitest";

import contract from "./ipc.contract.json";
import {
  AGENT_STATUSES,
  type AgentState,
  CLAUDE_CONSENTS,
  CLAUDE_INTEGRATION_ERROR_KINDS,
  CLAUDE_PLUGIN_STATUSES,
  type ClaudeChange,
  type ClaudeIntegrationPreview,
  type ClaudeConsent,
  type ClaudeIntegration,
  type ClaudeIntegrationError,
  type ClaudeIntegrationErrorKind,
  type ClaudePluginStatus,
  type NotificationClicked,
  type AgentStatus,
  applyWorkspaceDelta,
  COMMAND_ERROR_KINDS,
  type CommandError,
  type CommandErrorKind,
  type GitInfo,
  LAUNCH_FAILURE_KINDS,
  type LaunchFailure,
  type LaunchMode,
  type Layout,
  type LayoutNode,
  type NewSession,
  type PaneEdge,
  type PersistenceMode,
  type Project,
  type Session,
  SESSION_KINDS,
  SESSION_STATUSES,
  type SessionExit,
  type SessionKind,
  type SessionState,
  type SplitDirection,
  type SplitPlacement,
  type StartedSession,
  type StartSessionOptions,
  type WorkspaceDelta,
  type WorkspaceSnapshot,
} from "./ipc";

/**
 * Guards the types of `ipc.ts` against the Rust side.
 *
 * `ipc.contract.json` holds one example of everything the two sides
 * exchange. A Rust test builds it from the real types and fails when the
 * file differs (src-tauri/src/contract.rs). This file checks the same
 * examples against the TypeScript types:
 *
 * - The compiler makes sure each list of keys below names exactly the
 *   fields of its type (`satisfies Record<keyof T, true>`), and each list
 *   of values exactly the members of its union (`Exhaustive`).
 * - The tests make sure the examples have exactly those keys and values.
 *
 * So a field or a value that is added, removed or renamed on one side only
 * fails here or there.
 */

/** `true` only if the tuple type `Listed` names every member of `Union`. */
type Exhaustive<Union, Listed extends readonly Union[]> = [Union] extends [Listed[number]]
  ? true
  : never;

const SESSION_KEYS = {
  id: true,
  kind: true,
  cwd: true,
  liveCwd: true,
  command: true,
  title: true,
  name: true,
  autoTitle: true,
  project: true,
  git: true,
  state: true,
  terminalId: true,
  createdAt: true,
  lastActivityAt: true,
  agent: true,
  claudeSessionId: true,
  warning: true,
} satisfies Record<keyof Session, true>;

const PROJECT_KEYS = {
  key: true,
  name: true,
  root: true,
  kind: true,
} satisfies Record<keyof Project, true>;

const GIT_KEYS = {
  branch: true,
  commit: true,
  worktree: true,
} satisfies Record<keyof GitInfo, true>;

const EXITED_KEYS = {
  status: true,
  code: true,
  signal: true,
  failure: true,
} satisfies Record<keyof Extract<SessionState, { status: "exited" }>, true>;

const FAILURE_KEYS = {
  kind: true,
  message: true,
} satisfies Record<keyof LaunchFailure, true>;

const LAYOUT_KEYS = {
  root: true,
  focused: true,
  nextId: true,
} satisfies Record<keyof Layout, true>;

const PANE_KEYS = {
  type: true,
  id: true,
  session: true,
} satisfies Record<keyof Extract<LayoutNode, { type: "pane" }>, true>;

const SPLIT_KEYS = {
  type: true,
  id: true,
  direction: true,
  ratio: true,
  first: true,
  second: true,
} satisfies Record<keyof Extract<LayoutNode, { type: "split" }>, true>;

const SNAPSHOT_KEYS = {
  revision: true,
  sessions: true,
  projects: true,
  layout: true,
  recentFolders: true,
  persistence: true,
} satisfies Record<keyof WorkspaceSnapshot, true>;

const DELTA_KEYS = {
  revision: true,
  sessions: true,
  removed: true,
  sessionOrder: true,
  projects: true,
  layout: true,
  recentFolders: true,
  persistence: true,
} satisfies Record<keyof WorkspaceDelta, true>;

const STARTED_KEYS = {
  terminal: true,
  ackBatchBytes: true,
  windowsBuildNumber: true,
} satisfies Record<keyof StartedSession, true>;

const EXIT_KEYS = {
  code: true,
  signal: true,
  failure: true,
} satisfies Record<keyof SessionExit, true>;

const AGENT_KEYS = {
  status: true,
  waitingSince: true,
  since: true,
  stale: true,
} satisfies Record<keyof AgentState, true>;

const COMMAND_ERROR_KEYS = {
  command: true,
  kind: true,
  message: true,
} satisfies Record<keyof CommandError, true>;

const CLAUDE_INTEGRATION_KEYS = {
  claudeInstalled: true,
  plugin: true,
  version: true,
  consent: true,
  error: true,
} satisfies Record<keyof ClaudeIntegration, true>;

const CLAUDE_ERROR_KEYS = {
  kind: true,
  message: true,
} satisfies Record<keyof ClaudeIntegrationError, true>;

const CLAUDE_CHANGE_KEYS = {
  path: true,
  description: true,
} satisfies Record<keyof ClaudeChange, true>;

const CLAUDE_PREVIEW_KEYS = {
  changes: true,
  note: true,
} satisfies Record<keyof ClaudeIntegrationPreview, true>;

const NOTIFICATION_CLICKED_KEYS = {
  sessionId: true,
} satisfies Record<keyof NotificationClicked, true>;

const PROJECT_KINDS = ["git", "folder"] as const satisfies readonly Project["kind"][];
const LAUNCH_MODES = ["fresh", "resume"] as const satisfies readonly LaunchMode[];
const DIRECTIONS = ["row", "column"] as const satisfies readonly SplitDirection[];
const PLACEMENTS = ["before", "after"] as const satisfies readonly SplitPlacement[];
const EDGES = ["left", "right", "top", "bottom", "center"] as const satisfies readonly PaneEdge[];
const PERSISTENCE_MODES = [
  "primary",
  "secondary",
  "disabled",
  "memory",
  "failing",
] as const satisfies readonly PersistenceMode["mode"][];

// Each list names every member of its union, or this does not compile.
export const EXHAUSTIVE: [
  Exhaustive<SessionKind, typeof SESSION_KINDS>,
  Exhaustive<SessionState["status"], typeof SESSION_STATUSES>,
  Exhaustive<LaunchFailure["kind"], typeof LAUNCH_FAILURE_KINDS>,
  Exhaustive<Project["kind"], typeof PROJECT_KINDS>,
  Exhaustive<LaunchMode, typeof LAUNCH_MODES>,
  Exhaustive<SplitDirection, typeof DIRECTIONS>,
  Exhaustive<SplitPlacement, typeof PLACEMENTS>,
  Exhaustive<PaneEdge, typeof EDGES>,
  Exhaustive<PersistenceMode["mode"], typeof PERSISTENCE_MODES>,
  Exhaustive<CommandErrorKind, typeof COMMAND_ERROR_KINDS>,
  Exhaustive<AgentStatus, typeof AGENT_STATUSES>,
  Exhaustive<ClaudePluginStatus, typeof CLAUDE_PLUGIN_STATUSES>,
  Exhaustive<ClaudeConsent, typeof CLAUDE_CONSENTS>,
  Exhaustive<ClaudeIntegrationErrorKind, typeof CLAUDE_INTEGRATION_ERROR_KINDS>,
] = [true, true, true, true, true, true, true, true, true, true, true, true, true, true];

type Json = Record<string, unknown>;

function keysOf(value: unknown): string[] {
  return Object.keys(value as Json).sort();
}

function expectKeys(value: unknown, keys: Record<string, true>): void {
  expect(keysOf(value)).toEqual(Object.keys(keys).sort());
}

function expectSession(value: unknown): void {
  const session = value as Json;
  expectKeys(session, SESSION_KEYS);
  expectKeys(session.project, PROJECT_KEYS);
  if (session.git !== null) expectKeys(session.git, GIT_KEYS);
  const state = session.state as Json;
  expect(SESSION_STATUSES).toContain(state.status);
  if (state.status === "exited") {
    expectKeys(state, EXITED_KEYS);
    if (state.failure !== null) expectFailure(state.failure);
  } else {
    expect(keysOf(state)).toEqual(["status"]);
  }
  expect(SESSION_KINDS).toContain(session.kind);
  if (session.warning !== null) expectFailure(session.warning);
  if (session.agent !== null) {
    expectKeys(session.agent, AGENT_KEYS);
    expect(AGENT_STATUSES).toContain((session.agent as Json).status);
  }
}

function expectFailure(value: unknown): void {
  expectKeys(value, FAILURE_KEYS);
  expect(LAUNCH_FAILURE_KINDS).toContain((value as Json).kind);
}

function expectLayout(value: unknown): void {
  expectKeys(value, LAYOUT_KEYS);
  const visit = (node: Json): void => {
    if (node.type === "pane") {
      expectKeys(node, PANE_KEYS);
    } else {
      expect(node.type).toBe("split");
      expectKeys(node, SPLIT_KEYS);
      expect(DIRECTIONS).toContain(node.direction);
      visit(node.first as Json);
      visit(node.second as Json);
    }
  };
  visit((value as Json).root as Json);
}

describe("the contract with the Rust side", () => {
  it("has a snapshot with the fields of WorkspaceSnapshot", () => {
    const snapshot = contract.snapshot;

    expectKeys(snapshot, SNAPSHOT_KEYS);
    expect(snapshot.sessions.length).toBeGreaterThan(0);
    snapshot.sessions.forEach(expectSession);
    snapshot.projects.forEach((project) => {
      expectKeys(project, PROJECT_KEYS);
      expect(PROJECT_KINDS).toContain(project.kind);
    });
    expectLayout(snapshot.layout);
    // The examples cover every state, kind and nullable field.
    expect(new Set(snapshot.sessions.map((session) => session.state.status))).toEqual(
      new Set(SESSION_STATUSES),
    );
    expect(new Set(snapshot.sessions.map((session) => session.kind))).toEqual(
      new Set(SESSION_KINDS),
    );
    expect(snapshot.sessions.some((session) => session.git === null)).toBe(true);
    expect(snapshot.sessions.some((session) => session.git !== null)).toBe(true);
    expect(snapshot.sessions.some((session) => session.warning === null)).toBe(true);
    expect(snapshot.sessions.some((session) => session.warning !== null)).toBe(true);
    expect(snapshot.sessions.some((session) => session.agent === null)).toBe(true);
    expect(snapshot.sessions.some((session) => session.agent !== null)).toBe(true);
  });

  it("has deltas with the fields of WorkspaceDelta, set or null", () => {
    const [full, empty] = contract.deltas;

    for (const delta of contract.deltas) expectKeys(delta, DELTA_KEYS);
    full?.sessions.forEach(expectSession);
    expectLayout(full?.layout);
    full?.projects?.forEach((project) => {
      expectKeys(project, PROJECT_KEYS);
    });
    expect(empty).toMatchObject({
      sessions: [],
      removed: [],
      sessionOrder: null,
      projects: null,
      layout: null,
      recentFolders: null,
      persistence: null,
    });
  });

  it("can be applied as the types say", () => {
    // The cast is the claim under test: the checks above are what back it.
    const snapshot = contract.snapshot as unknown as WorkspaceSnapshot;
    const [full, empty] = contract.deltas as unknown as [WorkspaceDelta, WorkspaceDelta];

    const after = applyWorkspaceDelta(snapshot, full);

    expect(after.revision).toBe(full.revision);
    expect(after.sessions.map((session) => session.id)).toEqual(full.sessionOrder);
    expect(after.layout).toEqual(full.layout);
    expect(after.persistence).toEqual(full.persistence);
    expect(applyWorkspaceDelta(after, empty).sessions).toEqual(after.sessions);
  });

  it("lists the values of every enumeration", () => {
    expect(contract.enums).toEqual({
      sessionKind: SESSION_KINDS,
      sessionStatus: SESSION_STATUSES,
      projectKind: PROJECT_KINDS,
      launchMode: LAUNCH_MODES,
      direction: DIRECTIONS,
      placement: PLACEMENTS,
      edge: EDGES,
      agentStatus: AGENT_STATUSES,
      pluginStatus: CLAUDE_PLUGIN_STATUSES,
      consent: CLAUDE_CONSENTS,
      integrationErrorKind: CLAUDE_INTEGRATION_ERROR_KINDS,
    });
    expect(contract.persistence.map((persistence) => persistence.mode)).toEqual(PERSISTENCE_MODES);
    expect(contract.persistence.map(keysOf)).toEqual([
      ["mode"],
      ["mode"],
      ["mode", "reason"],
      ["mode"],
      ["mode", "reason", "since"],
    ]);
    contract.launchFailures.forEach(expectFailure);
    expect(contract.launchFailures.map((failure) => failure.kind)).toEqual(LAUNCH_FAILURE_KINDS);
  });

  it("has an error of every kind", () => {
    for (const error of contract.commandErrors) {
      expectKeys(error, COMMAND_ERROR_KEYS);
    }
    expect(contract.commandErrors.map((error) => error.kind)).toEqual(COMMAND_ERROR_KINDS);
    // Every kind of launch failure is also a kind of command error.
    for (const kind of LAUNCH_FAILURE_KINDS) expect(COMMAND_ERROR_KINDS).toContain(kind);
  });

  it("describes the Claude Code integration in each of its states", () => {
    const statuses = contract.claudeIntegration as Json[];
    expect(statuses.map((status) => status.plugin)).toEqual(CLAUDE_PLUGIN_STATUSES);
    for (const status of statuses) {
      // `version` and `error` are left out when there is none.
      const optional = new Set(["version", "error"]);
      for (const key of keysOf(status)) expect(Object.keys(CLAUDE_INTEGRATION_KEYS)).toContain(key);
      for (const key of Object.keys(CLAUDE_INTEGRATION_KEYS)) {
        if (!optional.has(key)) expect(keysOf(status)).toContain(key);
      }
      expect(CLAUDE_CONSENTS).toContain(status.consent);
      if (status.error !== undefined) expectKeys(status.error, CLAUDE_ERROR_KEYS);
    }
    expect(statuses.some((status) => status.version !== undefined)).toBe(true);
    expect(statuses.some((status) => status.error !== undefined)).toBe(true);
    const preview = contract.claudePreview as { changes: unknown[]; note?: unknown };
    expectKeys(preview, CLAUDE_PREVIEW_KEYS);
    expect(typeof preview.note).toBe("string");
    for (const change of preview.changes) expectKeys(change, CLAUDE_CHANGE_KEYS);
    expectKeys(contract.notificationClicked, NOTIFICATION_CLICKED_KEYS);
  });

  it("describes a started session and the exit of one", () => {
    expectKeys(contract.startedSession, STARTED_KEYS);

    const [plain, failed] = contract.exitEvents as Json[];
    // An ordinary exit leaves `failure` out; `startSession` turns that
    // into `null`.
    expect(keysOf(plain)).toEqual(["code", "event", "signal"]);
    expect(keysOf(failed)).toEqual(["event", ...Object.keys(EXIT_KEYS)].sort());
    expect(failed?.event).toBe("exit");
    expectFailure(failed?.failure);
  });

  it("sends requests the Rust side reads as intended", () => {
    // Typed here, and read by the Rust test: both sides agree on them.
    const createSession: NewSession[] = [
      { kind: "shell" },
      {
        kind: "background",
        cwd: "/work/app",
        command: "pnpm dev",
        name: "dev server",
        show: false,
      },
    ];
    const startSession: StartSessionOptions[] = [
      { cols: 120, rows: 40 },
      { mode: "resume", cols: 80, rows: 24 },
    ];

    expect(contract.requests).toEqual({ createSession, startSession });
  });
});
