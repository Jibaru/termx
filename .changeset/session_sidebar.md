---
default: minor
---

# Run many sessions in one window, listed in a sidebar and shown in split panes

- A sidebar lists every session, grouped by project (a repository and its
  worktrees together), with background commands in a section of their own.
  Each row shows the title (on up to two lines, the whole of it in a
  tooltip), the kind of session, the Git branch, whether
  it runs (or why it failed, such as "Claude Code is not installed" or
  "Folder no longer exists") and when it last printed something. The
  sidebar can be resized and hidden.
- Reorder sessions and projects by dragging them or with Alt+Up and
  Alt+Down; rename a session by double-clicking it or with F2. The name you
  give always wins over the title its program sets; clearing it gives that
  title back.
- Start a session from a quick dialog: Claude Code, a shell or a background
  command, in a recent folder or one you choose. A shortcut starts another
  Claude Code session in the project you are in without asking anything.
- Split the main area side by side or stacked, drag the dividers, move
  between panes with the keyboard, and drop a session from the sidebar onto
  a pane.
- After a restart, your sessions come back without running: resume them one
  by one, or all at once from the banner.
- termx says so when your changes are not being saved, for instance on a
  full disk or in a second termx window.
- Reloading the window, or the window coming back after a crash, no longer
  stops anything: running sessions carry on and their terminals are
  repainted. Restarting a session that runs asks first.
- On macOS, termx has a menu bar with the main actions and their
  shortcuts. Cmd+W closes the focused pane, never the window, and closing
  the window or quitting while sessions run asks first.
