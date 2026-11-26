---
default: minor
---

# A real terminal

termx now opens your login shell in a real terminal, in your home directory,
instead of a placeholder.

- Colours (16, 256 and 24-bit), emoji and wide characters, clickable links
  (Cmd+click on macOS, Ctrl+click elsewhere) and 10,000 lines of scrollback.
- Copy and paste with Cmd+C and Cmd+V on macOS, and with Ctrl+Shift+C and
  Ctrl+Shift+V on Windows and Linux. Programs can copy to your clipboard, but
  never read it.
- Shift+Enter inserts a new line in Claude Code without running
  `/terminal-setup`, and on macOS the Option key works as Meta.
- A program that prints faster than the screen can draw is slowed down
  instead of using more and more memory.
- When the shell exits, the terminal shows its exit code, and Enter starts a
  new one.
- Quitting termx ends the shells it started.

The dark theme and the font are fixed for now; settings come later. On macOS
the Option key no longer types the alternate characters of your keyboard
layout in the terminal.
