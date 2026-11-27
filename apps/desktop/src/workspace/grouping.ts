import type { Project, Session, SessionId, WorkspaceSnapshot } from "../lib/ipc";

/**
 * How the sidebar arranges the sessions: one group per project, in the
 * order of the projects, then the background commands in a section of
 * their own. Within a group, sessions keep the order of the workspace.
 *
 * Worktrees need nothing here: the Rust side gives the session of a linked
 * worktree the project of its main repository.
 */

export interface ProjectGroup {
  project: Project;
  sessions: Session[];
}

export interface SidebarModel {
  groups: ProjectGroup[];
  /** The background commands, whatever their project. */
  background: Session[];
  /** Every session in the order the sidebar shows them. */
  order: SessionId[];
}

export function buildSidebar(
  workspace: Pick<WorkspaceSnapshot, "sessions" | "projects">,
): SidebarModel {
  const byProject = new Map<string, Session[]>();
  const background: Session[] = [];
  for (const session of workspace.sessions) {
    if (session.kind === "background") {
      background.push(session);
      continue;
    }
    const list = byProject.get(session.project.key);
    if (list) list.push(session);
    else byProject.set(session.project.key, [session]);
  }

  const groups: ProjectGroup[] = [];
  for (const project of workspace.projects) {
    const sessions = byProject.get(project.key);
    if (!sessions) continue;
    groups.push({ project, sessions });
    byProject.delete(project.key);
  }
  // A project the list does not name yet (a delta in between) goes last.
  for (const sessions of byProject.values()) {
    const first = sessions[0];
    if (first) groups.push({ project: first.project, sessions });
  }

  const order = [
    ...groups.flatMap((group) => group.sessions.map((session) => session.id)),
    ...background.map((session) => session.id),
  ];
  return { groups, background, order };
}

/**
 * What to pass to `reorderSession` (or `reorderProject`) to put `moved`
 * at `index` of `siblings`, the list it is shown in (which contains it).
 * The order of the workspace is global, so a session moved to the end of
 * its group goes to the end of everything (`null`): it is still last
 * among its siblings.
 *
 * `null` as a result means "to the end"; `undefined` means nothing moves.
 */
export function beforeFor<T>(
  siblings: readonly T[],
  moved: T,
  index: number,
): T | null | undefined {
  const from = siblings.indexOf(moved);
  if (from === -1) return undefined;
  const others = siblings.filter((item) => item !== moved);
  const to = Math.max(0, Math.min(index, others.length));
  if (to === from) return undefined;
  return others[to] ?? null;
}

/**
 * Where `moved` lands when it is dropped on `target`, before or after it.
 * Returns the index for `beforeFor`.
 */
export function dropIndex<T>(
  siblings: readonly T[],
  moved: T,
  target: T,
  position: "before" | "after",
): number {
  const others = siblings.filter((item) => item !== moved);
  const at = others.indexOf(target);
  if (at === -1) return siblings.indexOf(moved);
  return position === "before" ? at : at + 1;
}
