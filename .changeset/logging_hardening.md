---
default: patch
---

# More robust and more careful logging

The log now holds up better when things go badly wrong, and is more careful
about what ends up in it.

- A repeating error no longer pushes the error that started it out of the log:
  repeats are counted instead of written, and the first warnings and errors of
  each launch are kept in a file of their own.
- A slow or stalled disk can no longer freeze termx while it logs an error.
- A crash is recorded even when the log file itself cannot be written.
- Tokens, passwords and keys that turn up in an error message are replaced by
  `<redacted>`, and your user name is recognised in more spellings. Ordinary
  text that only resembles a home directory, such as `src/home/Home.tsx`, is
  no longer changed.
- Log files of a termx window that is still running are never cleaned up by
  another one, and files that are links or belong to someone else are never
  exported.

[Logging and diagnostics](https://github.com/Jibaru/termx/blob/main/docs/logging.md)
describes exactly what is guaranteed and what is best effort. As before,
nothing is sent anywhere, and you should skim a diagnostics bundle before
attaching it to a public issue.
