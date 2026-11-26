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

The files are named `termx-<UTC timestamp>-<run id>-<sequence>.log`, for
example `termx-20261126T190411Z-5f0c2a9be771-000.log`. Every launch of termx
writes its own files, so several termx windows running at once never write to
the same file. The newest file is the one being written.

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

Logging starts before anything else, so a termx that fails to open its window
still leaves a record.

## Privacy

The log is meant to be shared. This section separates what the code enforces
from what is a rule the code is written to follow, because only the first
kind can be promised.

**Skim the bundle before you attach it.** It is a zip of plain text files.
Issues on GitHub are public, and no filter can recognise everything that is
private to you.

### Enforced by the code

These hold for every record, whatever wrote it:

- **Your home directory is written as `~`**, in the spellings termx knows it
  can take: with `/` or `\`, with mixed or doubled separators,
  percent-encoded as in URLs, as seen from WSL (`/mnt/c/Users/...`), and in
  any letter case on Windows and macOS.
- **The user name in any other home-shaped path is written as `<user>`**:
  `/Users/<user>`, `/home/<user>`, `C:\Users\<user>`. This also covers other
  accounts on the machine.
- **Your user name on its own is written as `<user>`** when it appears as a
  whole word, provided it has at least four characters and is not a generic
  account name or a common word (`admin`, `user`, `root`, `error`, ...).
  Shorter and generic names are left alone outside paths, because replacing
  them would garble ordinary text.
- **Values thrown by the interface that are not errors are never written.**
  Only their type and, for objects, the names of their top-level keys are
  recorded. For errors, only the name, the message and the stack are.
- **Nothing is collected about the machine or the user** beyond the versions
  and platform in the startup record: no host name, user name, hardware
  identifier or network address. The `run` identifier is random and changes
  with every launch.
- **Only termx's own log files are read, exported or deleted**: regular files
  in the log directory with exactly the name pattern above. Symbolic links
  are never followed.
- **The diagnostics bundle is only ever written to your Downloads folder.**
- **Text values are capped** at 16 KiB, so one record cannot dump a large
  amount of data.
- **Release builds do not carry the build machine's paths.** They are built
  with `--remap-path-prefix` (see
  [`release-rustflags.sh`](../.github/scripts/release-rustflags.sh)), so
  backtraces show paths such as `./crates/termx-core/src/...` and
  `/cargo/registry/...`. Builds you make yourself contain your own paths
  unless you use the same flags; the home directory rules above still apply
  to them when they are logged.

### Rules the code follows, which no filter can enforce

termx is written so that these never reach a logging call, and contributions
are reviewed against them ([CONTRIBUTING.md](../CONTRIBUTING.md#logging-conventions)),
but nothing in the logger can tell such data from any other text:

- **Terminal output is not logged.** Nothing a program prints in a session.
- **Keystrokes and other input are not logged.**
- **The values of environment variables are not logged.** Only their names
  may appear.
- **The contents of files are not logged.**

### Best effort

- Error messages produced by the operating system, by libraries or by the
  interface are recorded as they are, after the replacements above. They can
  mention a file name, a path outside your home directory or another detail
  of your setup.
- The replacements are pattern matching. A home directory in an unusual
  location is only recognised for the current user. For another account, a
  user name that contains spaces is only cut at the first space.
- Project and directory names under your home directory stay readable:
  `~/work/client-name` is written exactly like that.

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

| Target            | Records                                                                    |
| ----------------- | -------------------------------------------------------------------------- |
| `termx::startup`  | The startup record.                                                        |
| `termx::panic`    | Crashes. Fields: `message`, `location`, `thread`, `backtrace`.             |
| `termx::command`  | Failed commands. Fields: `command`, and `error` or `panic`.                |
| `termx::frontend` | Interface errors. Fields: `source`, and when known `stack`, `command`.     |
| `termx::log`      | Notes of the logger about itself, such as how many records it had to drop. |

For example, to list the errors of every log file on macOS:

```sh
cat ~/Library/Logs/termx/termx-*.log | jq -c 'select(.level == "ERROR")'
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

## What is guaranteed to be written

The record that explains a crash is usually the last one before it, so:

- **`WARN` and `ERROR` records are written before the code that logged them
  continues.** They are in the file even if termx is killed, aborts or exits
  immediately afterwards, and they are never dropped.
- **Crash records are written by the crashing thread itself** and flushed to
  the disk, after everything that was still waiting.
- **`INFO`, `DEBUG` and `TRACE` records are written by a background thread**,
  so that frequent logging never slows down the interface or the terminals.
  They reach the file within moments, and always before the next warning or
  error. If termx is killed outright, the last few of them can be missing. If
  they arrive faster than they can be written, the excess is dropped and a
  `termx::log` record says how many were.
- **When termx exits normally**, everything is written and flushed first.

Interface errors are rate limited, so that an error that repeats on every
frame cannot fill the log:

- Each level has its own budget: 20 errors at once and then one per second,
  10 warnings and 10 other records at once and then one every two seconds.
  Noise at a low level cannot use up the room for errors.
- An error that was not seen before gets through even when repeats of another
  error have used up the budget (up to 50 more at once, then one per second).
- Every dropped record is counted. A `termx::frontend` record with the counts
  per level is written with the next record that gets through, and when termx
  exits.

## Retention

The log cannot grow without bound:

- A new file is started when the current one reaches **5 MiB**.
- Files not written to for **14 days** are deleted.
- All log files together take at most **25 MiB**, and there are at most
  **100** of them: the oldest are deleted first.
- The **3** newest files are kept whatever their age, so a wrong system clock
  cannot wipe the log.

The rules are applied when termx starts, every time a new file is started and
regularly while it writes. They cover the files of every running termx, so
the limits hold for the directory as a whole (several instances writing at the
same moment can overshoot by a few hundred kilobytes for an instant).

If a log file is deleted while termx runs, termx notices on the next record
and starts a new one. If no new file can be started (a full disk, for
example), termx keeps writing to the current file and empties it when it
reaches twice its limit.

## Sending logs with a bug report

1. In the termx window, click **Export diagnostics**. termx writes
   `termx-diagnostics-<UTC timestamp>.zip` to your Downloads folder, opens
   that folder and shows the path.
2. **Open the zip and skim it.** See [Privacy](#privacy) for what the code
   guarantees and what it cannot.
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
  "log_files": [{ "name": "termx-20261126T190411Z-5f0c2a9be771-000.log", "bytes": 18234 }]
}
```
