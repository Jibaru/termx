---
default: minor
---

# Error logging and a diagnostics bundle

termx now keeps a log file on your machine and records errors and crashes in
it, so that a problem can be investigated after it happened. Nothing is sent
anywhere: you decide whether to share the log.

- **Open log folder** shows the log files in your file manager.
- **Export diagnostics** saves a zip with the log files and the versions of
  termx and of your system to your Downloads folder, ready to attach to a bug
  report.

The log never contains terminal output, typed input, environment variable
values or file contents, and your home directory is written as `~`. See
[Logging and diagnostics](https://github.com/Jibaru/termx/blob/main/docs/logging.md) for the details.
