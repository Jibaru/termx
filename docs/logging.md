# Logging and diagnostics

termx keeps a log on your machine so that, when something goes wrong, there is
a record of it that you can send along with a bug report.

**Nothing is ever sent anywhere automatically.** termx has no telemetry, no
crash reporter and no account. The log files stay on your disk until you
decide to share them.

## Where the logs are

| Platform | Directory                                                    |
| -------- | ------------------------------------------------------------ |
| macOS    | `~/Library/Logs/termx`                                       |
| Linux    | `$XDG_STATE_HOME/termx/logs`, or `~/.local/state/termx/logs` |
| Windows  | `%LOCALAPPDATA%\termx\logs`                                  |

The file being written is `termx.log`. Older files are named
`termx-<UTC timestamp>-<counter>.log`.

**Open log folder** in the termx window opens this directory in your file
manager. Setting the `TERMX_LOG_DIR` environment variable to a directory makes
termx log there instead.

## What is recorded

- A startup record: the termx version, the operating system and its version,
  the CPU architecture, the webview version and the locale.
- Errors returned by the application's internal commands.
- Crashes (Rust panics) on any thread, with the message, the source location
  and a backtrace.
- Errors in the interface: uncaught JavaScript errors, unhandled promise
  rejections, rendering errors and failed calls to the backend.
- At the more verbose levels, lifecycle events that help to follow what the
  application was doing.

## Privacy

The log is meant to be shared, so termx follows these rules:

- **Terminal output is never logged.** Nothing a program prints in a session
  ends up in the log.
- **Keystrokes and other input are never logged.**
- **The values of environment variables are never logged.** Only their names
  may appear.
- **The contents of files are never logged.**
- **Your home directory is replaced by `~`.** Every record passes through a
  filter that does this for the message and for every text field, so a path
  such as `/home/ana/projects/site` is written as `~/projects/site` and your
  user name stays out of the log. Paths outside your home directory are
  written as they are.
- **No machine or user identifiers.** There is no host name, user name,
  hardware identifier or network address in the log or in the diagnostics
  bundle. The `run` identifier is random and changes with every launch.

Error messages produced by the operating system or by libraries are recorded
as they are (after the home directory filter). The log is plain text: if you
want to be sure, read it before sharing it.

## Format

The log is in [JSON Lines](https://jsonlines.org) format: every line is one
JSON object, which is readable as text and easy to process with tools such as
`jq`.

```json
{
  "ts": "2026-11-26T19:04:11.482Z",
  "level": "ERROR",
  "run": "5f0c2a9be771",
  "target": "termx::command",
  "msg": "command failed",
  "fields": {
    "command": "export_diagnostics",
    "error": "~/Downloads: Permission denied (os error 13)"
  }
}
```

| Key      | Meaning                                                                        |
| -------- | ------------------------------------------------------------------------------ |
| `ts`     | Time of the record in UTC, with milliseconds (RFC 3339).                       |
| `level`  | `ERROR`, `WARN`, `INFO`, `DEBUG` or `TRACE`.                                   |
| `run`    | Random identifier of the launch. All records of one launch share it.           |
| `target` | The part of termx that wrote the record, usually a Rust module path.           |
| `msg`    | What happened.                                                                 |
| `span`   | Only when present: the operations in progress, outermost first, joined by `>`. |
| `fields` | Structured details. Always present, possibly empty.                            |

The keys always appear in this order. Text values longer than 16 KiB are cut
and end in `…[truncated]`.

Some targets are fixed:

| Target            | Records                                                                |
| ----------------- | ---------------------------------------------------------------------- |
| `termx::startup`  | The startup record.                                                    |
| `termx::panic`    | Crashes. Fields: `message`, `location`, `thread`, `backtrace`.         |
| `termx::command`  | Failed commands. Fields: `command`, `error`.                           |
| `termx::frontend` | Interface errors. Fields: `source`, and when known `stack`, `command`. |

For example, to list the errors of the current log file:

```sh
jq -c 'select(.level == "ERROR")' ~/Library/Logs/termx/termx.log
```

## Levels

The default level is `info`. Set the `TERMX_LOG` environment variable before
starting termx to change it:

```sh
TERMX_LOG=debug termx
```

| Value   | Records                                               |
| ------- | ----------------------------------------------------- |
| `off`   | Nothing, except crashes.                              |
| `error` | Failures.                                             |
| `warn`  | Also unexpected situations that termx recovered from. |
| `info`  | Also startup, shutdown and other lifecycle events.    |
| `debug` | Also details that help when investigating a problem.  |
| `trace` | Everything.                                           |

An unknown value is ignored and the default is used. Crashes are recorded at
every level. A configuration file setting will be added later; the environment
variable will keep working.

Debug builds also print the records to stderr.

## Retention

The log cannot grow without bound:

- `termx.log` is rotated when it reaches **5 MiB**.
- Rotated files older than **14 days** are deleted.
- All log files together take at most **25 MiB**: the oldest rotated files
  are deleted first.

The rules are applied when termx starts and every time a file is rotated. Only
files named `termx.log` or `termx-*.log` in the log directory are ever
deleted.

Interface errors are rate limited (a burst of 20 records, then one per second)
so that an error that repeats on every frame cannot fill the log. A record
notes how many were dropped.

Records are written by a background thread so that logging never slows down
the interface or the terminals. In the unlikely case that this thread cannot
keep up, records are dropped rather than making the application wait.

## Sending logs with a bug report

1. In the termx window, click **Export diagnostics**. termx writes
   `termx-diagnostics-<UTC timestamp>.zip` to your Downloads folder and opens
   that folder.
2. Optionally, open the zip and check what it contains (see below).
3. Open a [bug report](https://github.com/Jibaru/termx/issues/new?template=bug_report.yml)
   and drag the zip file into the **Diagnostics bundle** field.

If termx does not start at all, zip the log directory by hand and attach that
instead.

The bundle contains:

| Entry              | Content                                                                     |
| ------------------ | --------------------------------------------------------------------------- |
| `logs/`            | The log files described above, exactly as they are on disk.                 |
| `system-info.json` | Versions and platform, as in the startup record. Nothing else is collected. |

```json
{
  "generated_at": "2026-11-26T19:10:02.118Z",
  "run_id": "5f0c2a9be771",
  "log_level": "info",
  "app_version": "0.1.0",
  "core_version": "0.1.0",
  "os": "linux",
  "os_version": "Ubuntu 24.04",
  "arch": "x86_64",
  "webview_version": "2.46.5",
  "locale": "en-US",
  "debug_build": false,
  "log_files": [{ "name": "termx.log", "bytes": 18234 }]
}
```

Issues on GitHub are public. The privacy rules above are there so that the
bundle is safe to attach, but it is your data: look inside first if in doubt.
