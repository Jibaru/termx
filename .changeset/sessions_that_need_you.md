---
default: minor
---

# See which Claude Code session needs you

With many Claude Code sessions open, termx now tells you which ones are
waiting for you, once you let it install its Claude Code plugin.

- Each Claude session shows whether its agent is working, waiting for your
  permission, waiting for your answer, or done, on its row and in the header
  of its pane. Sessions that wait stand out: a magenta bar, a bold title,
  and a count on their project when it is collapsed.
- **Needs you**, at the top of the sidebar, lists the sessions that wait,
  the one waiting longest first. Shift+Cmd+A on macOS, Ctrl+Shift+A on
  Windows and Linux, goes to it; pressed again, to the next one.
- A notification tells you when a session starts to wait while you are
  looking elsewhere; a burst is one notification, and clicking it shows the
  session. The app icon shows how many sessions wait. Notifications can be
  turned off, and turned on for finished sessions, at the bottom of the
  sidebar.
- The first time termx finds Claude Code it asks before installing anything,
  with the exact list of what the plugin adds. Your own hooks and settings
  are not changed, and the plugin can be updated or removed from the bottom
  of the sidebar.
