---
default: patch
---

# Ctrl+C interrupts programs on Windows however termx was started

When termx was started by a program that ignores Ctrl+C (some launchers,
`start /b`, a process started in a new process group), its terminals
inherited that, and Ctrl+C interrupted nothing in them. termx now gives its
terminals the default handling of Ctrl+C back.
