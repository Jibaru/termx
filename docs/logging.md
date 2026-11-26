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
example `termx-20261126T190411Z-5f0c2a9be771-000.log`. The timestamp is when
that launch of termx started, and the sequence number counts its files. Every
launch writes its own files, so several termx windows running at once never
write to the same file.

A launch can have three more files next to those:

| File                              | What it is                                                                                                                                              |
| --------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `termx-<...>-<run id>-errors.log` | A copy of the first 200 warnings and errors of the launch. It is never deleted while that termx runs, so what went wrong first is not pushed out later. |
| `termx-<...>-<run id>-crash.log`  | A crash record that could not be written to the log itself because the log file was busy. Rare.                                                         |
| `termx-<run id>.lock`             | Empty. termx keeps it locked while it runs, which tells other termx windows that its log files are in use. It is not part of the diagnostics bundle.    |

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
  can take: with `/` or `\`, with mixed or repeated separators,
  percent-encoded once or twice as in URLs, as seen from WSL
  (`/mnt/c/Users/...`), with accented letters composed or decomposed, and in
  any letter case on Windows and macOS.
- **The user name in any other home-shaped path is written as `<user>`**:
  `/Users/<user>`, `/home/<user>`, `C:\Users\<user>`. This also covers other
  accounts on the machine. Such a path is recognised where a path can start:
  at the beginning of a text or after white space, a quote, a bracket, `=`,
  `:`, `,`, `;` or `file://`.
- **Values thrown by the interface that are not errors are never written.**
  Only their type is recorded and, for objects, the number of keys and the
  names of the keys that are plainly field names (letters and underscores).
  For errors, only the name, the message and the stack are.
- **Nothing is collected about the machine or the user** beyond the versions
  and platform in the startup record: no host name, user name, hardware
  identifier or network address. The `run` identifier is random and changes
  with every launch.
- **Only termx's own log files are read, exported or deleted**: regular files
  in the log directory with exactly the name patterns above. Symbolic links
  are never followed, and the log directory itself must be a real directory.
  On Linux and macOS a file must also belong to you and have no other hard
  link; Windows does not tell termx either of those.
- **The diagnostics bundle is only ever written to your Downloads folder.**
- **Text values are capped** at 16 KiB, so one record cannot dump a large
  amount of data.

### Best effort: replaced when recognised

These are done by pattern matching on every record. They catch the common
cases; they cannot catch everything, and they sometimes replace something
innocent.

- **Your user name on its own is written as `<user>`** when it stands as a
  whole word, in any letter case, percent-encoded or not. To keep ordinary
  text readable:
  - names of fewer than four characters, generic account names and common
    words (`admin`, `user`, `node`, `command`, `code`, ...) are never
    replaced outside a path;
  - names of four or five characters are only replaced where a user name is
    expected: next to a path separator or an `@`, or as the value of
    something called `user`, `login`, `owner`, `account` or `author`;
  - longer names are replaced everywhere.

  If your user name is an ordinary word that is not on termx's list, that
  word is replaced wherever it appears. If it is short or on the list, it is
  not replaced outside paths.

- **What looks like a secret is written as `<redacted>`**: tokens with a
  well-known prefix (GitHub, GitLab, Slack, OpenAI and Anthropic style
  `sk-` keys, Stripe, AWS access key ids, Google API keys, npm, Hugging
  Face), JSON Web Tokens, what follows `Bearer` or `Basic`, the value given
  to a name such as `password`, `secret`, `token`, `api_key`, `authorization`
  or `cookie`, and any other run of 32 or more letters and digits that looks
  random. A secret in another shape is not recognised. In the other
  direction, hashes (a Git commit id, a checksum) are replaced too.
- **A home directory in an unusual location** is only recognised for the
  current user. For another account, a user name that contains spaces is only
  cut at the first space.

### Planned, not done yet

- **Paths of the machine that built termx.** A program contains the source
  paths of the machine it was built on, and they show up in crash
  backtraces. There are no official builds of termx yet. When there are, they
  are to be built with `--remap-path-prefix`; the flags are ready in
  [`release-rustflags.sh`](../.github/scripts/release-rustflags.sh), and the
  release workflow has a note where to use them, but nothing applies them
  today. Until then, and in any build you make yourself, backtraces contain
  the builder's paths, with the home directory rules above applied to them
  when they are logged (`/Users/<user>/.cargo/registry/...`).

### Rules the code follows, which no filter can enforce

termx is written so that these never reach a logging call, and contributions
are reviewed against them ([CONTRIBUTING.md](../CONTRIBUTING.md#logging-conventions)),
but nothing in the logger can tell such data from any other text:

- **Terminal output is not logged.** Nothing a program prints in a session.
- **Keystrokes and other input are not logged.**
- **The values of environment variables are not logged.** Only their names
  may appear.
- **The contents of files are not logged.**

### Not filtered

- Error messages produced by the operating system, by libraries or by the
  interface are recorded as they are, after the replacements above. They can
  mention a file name, a path outside your home directory or another detail
  of your setup.
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

| Target            | Records                                                                                                  |
| ----------------- | -------------------------------------------------------------------------------------------------------- |
| `termx::startup`  | The startup record.                                                                                      |
| `termx::panic`    | Crashes. Fields: `message`, `location`, `thread`, `backtrace`, and `command` when a command was running. |
| `termx::command`  | Failed commands. Fields: `command`, `error`.                                                             |
| `termx::frontend` | Interface errors. Fields: `source`, and when known `stack`, `command`.                                   |
| `termx::log`      | Notes of the logger about itself: how many records it dropped or wrote late.                             |

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

Debug builds also print the records to stderr, as they are: the replacements
described under [Privacy](#privacy) only apply to the log files.

## What is guaranteed to be written

The record that explains a crash is usually the last one before it. At the
same time, logging must never freeze the interface or a terminal, even when
the disk does. termx resolves that as follows.

- **`WARN` and `ERROR` records are written before the code that logged them
  continues.** A background thread does the writing, and the code that logs
  waits for it, which normally takes well under a millisecond. The record is
  then in the file even if termx is killed, aborts or exits immediately
  afterwards.
- **The wait is limited to 100 ms.** If the disk does not answer in that time
  (a network drive that hangs, a laptop waking up), termx carries on, the
  record stays queued and is written as soon as the disk answers, and a
  `termx::log` record says how many were written late. While the disk is
  known to be stalled, nothing waits at all. This is the one case in which a
  warning or error can be lost: termx crashing while the disk is stalled. If
  the disk stays stalled long enough for more than about 32,000 of them to
  queue up, the rest are dropped and counted.
- **Repeats are counted, not written.** Warnings and errors that come from the
  same place in the code with the same message, numbers and identifiers
  aside, are written the first 5 times in 10 seconds. After that they are
  counted, and one record with `"repeated": <count>` and the message says how
  many more there were, when the 10 seconds are over, when the log is flushed
  and when termx exits. What a crash in the middle can lose is that count,
  not the first occurrences.
- **The first 200 warnings and errors of a launch are kept twice**: in the log
  and in the `-errors.log` file, which that launch never deletes. An error
  that causes a flood of other errors is still there afterwards.
- **Crash records are written by the crashing thread itself** and flushed to
  the disk, after everything that was still waiting. If it cannot get the log
  file within 0.3 s, it writes the record to the `-crash.log` file and to
  stderr instead.
- **`INFO`, `DEBUG` and `TRACE` records are not waited for.** They reach the
  file within moments, and always before the next warning or error. If termx
  is killed outright, the last few of them can be missing. If they arrive
  faster than they can be written (more than about 8,000 waiting), the excess
  is dropped and a `termx::log` record says how many were.
- **When termx exits normally**, everything is written and flushed first.

### Limits on errors from the interface

Errors of the interface are rate limited before they reach the log, so that
an error that repeats on every frame cannot fill it. The interface applies
the limit first, per 10 seconds, and the backend applies its own again:

|                                            | In the interface, per 10 s | In the backend                |
| ------------------------------------------ | -------------------------- | ----------------------------- |
| Errors                                     | 20                         | 20 at once, then 1 per second |
| Warnings                                   | 10                         | 10 at once, then 1 per 2 s    |
| Other records                              | 10                         | 10 at once, then 1 per 2 s    |
| Errors not seen before, per source, on top | 10                         | 10 at once, then 1 per second |

- Each level has its own budget, so noise at a low level cannot use up the
  room for errors.
- An error that was not seen before gets through even when repeats have used
  up the error budget. Two errors are the same when they come from the same
  source (uncaught error, unhandled rejection, rendering, a failed call) and
  have the same message once numbers and identifiers are ignored, so an error
  that carries a counter is one error, not thousands.
- The extra budget is per source, so a source that produces an endless stream
  of different errors only uses up its own. At most 32 sources have their
  own; any more share one.
- Every dropped record is counted. The interface logs its count with the next
  record that gets through. The backend writes a `termx::frontend` record with
  the counts per level at most every 5 seconds, and when termx exits.

These limits bound the damage; they do not make loss impossible. More
different errors than a budget allows are dropped, and only counted.

## Retention

The log cannot grow without bound:

- A new file is started when the current one reaches **5 MiB**.
- Files not written to for **14 days** are deleted.
- All log files together take at most **25 MiB**, and there are at most
  **100** of them: the oldest are deleted first.
- The **3** newest files are kept whatever their age, so a wrong system clock
  cannot wipe the log.

The rules are applied when termx starts, every time a new file is started and
regularly while it writes, with these exceptions:

- **A running termx never has its files deleted by another one.** Each
  running termx cleans up its own files and those of termx processes that
  have ended, and counts the files of the others against the limits. Several
  busy instances at once can therefore exceed 25 MiB together, by up to
  5 MiB for each additional instance.
- **A file dated in the future** (written while the system clock was wrong)
  does not count as one of the newest files, and is the first to go when room
  is needed.
- The `-errors.log` and `-crash.log` files of a launch are kept for as long
  as that termx runs. Afterwards they are ordinary old files.

If a log file is deleted, or replaced by another file, while termx runs,
termx notices before the next write and starts a new one. If no new file can
be started (a full disk, for example), termx keeps writing to the current
file and empties it when it reaches twice its limit.

## Sending logs with a bug report

1. In the termx window, click **Export diagnostics**. termx writes
   `termx-diagnostics-<UTC timestamp>.zip` to your Downloads folder, opens
   that folder and shows the path.
2. **Open the zip and skim it before attaching it.** The replacements are
   best effort. See [Privacy](#privacy) for what the code guarantees and what
   it cannot.
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
