---
default: minor
---

# See what each Claude Code session is doing

With the termx plugin for Claude Code, each session shows whether Claude
is working, waiting for your permission, waiting for your answer, or done,
and termx can notify you and badge its icon when sessions wait. termx asks
before installing the plugin, shows exactly what it adds to your Claude
Code settings (two entries; your own hooks and settings stay as they are),
and can remove it again. The hooks send termx only the name of each event
and a few identifiers, never your prompts, commands or files, and do
nothing outside termx. Sessions also resume the right conversation after
`/clear` or `/resume`.
