---
default: minor
---

# termx remembers its window and its sessions

termx now keeps a small state file on your machine, so that it starts the
way you left it.

- The window opens with the size and position it had, unless that would put
  it on a display that is no longer connected.
- The session you had on screen is shown again, waiting for you: press
  Enter to start it in the same folder. Nothing starts on its own, so a
  background command or Claude Code never runs again behind your back.
  Programs are not kept running while termx is closed, and nothing that was
  printed in a terminal is stored.
- If Claude Code is not installed, a Claude Code session says so in plain
  words instead of showing a dead terminal. If your shell profile keeps the
  command from running (for example a `.zshrc` that starts another shell),
  termx says that instead of leaving you at a bare prompt.
- Closing a session or quitting termx ends everything the session started,
  including programs that ignore the hang-up signal and those started in
  the background.

Choosing a folder, running several sessions side by side and the sidebar
that lists them come next: this release lays the ground for them.

[What termx remembers](docs/state.md) says where the file is and what is in
it.
