---
default: minor
---

# termx remembers its window and its sessions

termx now keeps a small state file on your machine, so that it starts the
way you left it.

- The window opens with the size and position it had, unless that would put
  it on a display that is no longer connected.
- The session you had on screen is offered again: termx starts a new shell
  for it in the same folder. Programs are not kept running while termx is
  closed, and nothing that was printed in a terminal is stored.
- If Claude Code is not installed, a Claude Code session says so in plain
  words instead of showing a dead terminal.

Choosing a folder, running several sessions side by side and the sidebar
that lists them come next: this release lays the ground for them.

[What termx remembers](docs/state.md) says where the file is and what is in
it.
