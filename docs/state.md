# What termx remembers

termx keeps one small file on your machine, so that it starts the way you
left it: the same sessions, in the same order and layout, with the window
where it was.

**The file never leaves your machine.** termx has no telemetry and no
account. Nothing is synchronised anywhere.

## Where the file is

| Platform | File                                                                     |
| -------- | ------------------------------------------------------------------------ |
| macOS    | `~/Library/Application Support/termx/state.json`                         |
| Linux    | `$XDG_STATE_HOME/termx/state.json`, or `~/.local/state/termx/state.json` |
| Windows  | `%LOCALAPPDATA%\termx\state.json`                                        |

Setting the `TERMX_STATE_DIR` environment variable to a directory makes termx
keep its state there instead. That is how to try termx without touching your
own state, or to run a second, independent termx.

On macOS and Linux the file can only be read by your user account.

## What is in it

- **Your sessions**: for each one its kind (Claude Code, shell or background
  command), its folder, the name you gave it, the last title its program set,
  its project and Git branch as last seen, when it was created and last
  active and, once known, the identifier of its Claude Code conversation. For
  a background command, also its command line.
- **Their order**, and the order of the projects.
- **The layout**: how the main area is split into panes, and which session
  each pane shows.
- **The folders you recently started sessions in**: the 20 most recent.
- **The window**: its size, its position and whether it was maximized.

The command line of a background command is stored exactly as you typed it.
If it contains a secret (a token passed as an argument), that secret is in
the file: prefer an environment variable or a configuration file of the tool
for such values.

## What is never in it

- **Nothing that was printed in a terminal.** Scrollback is not stored.
- **Nothing that was typed.**
- **No environment variables.**

## What happens when termx starts again

Programs are not kept running while termx is closed. Each session comes back
_dormant_: it is listed, in its pane, and nothing runs until it is resumed.

| Kind               | Resuming it                                                                                                                                                       |
| ------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Claude Code        | Runs `claude --resume <conversation>` when termx knows the conversation, otherwise `claude --continue`, which resumes the most recent conversation of the folder. |
| Shell              | Starts a new shell in the same folder.                                                                                                                            |
| Background command | Runs the same command again in the same folder.                                                                                                                   |

Claude Code and background commands are started by your shell, the same way
it starts for a terminal, so they find what your shell profile puts on
`PATH`.

## Two termx running at once

The first termx to start owns the state: it restores it and saves it. A
second one started while the first is running starts empty, offers the same
recent folders, and **does not save anything**: what you open in it is not
there after a restart. This keeps two instances from resuming the same
Claude Code conversation twice, and from overwriting each other's sessions.

To run two independent instances, give the second one its own
`TERMX_STATE_DIR`.

## If the file cannot be used

termx always starts, and never throws your state away without keeping a
copy next to it:

| The file                            | What termx does                                                          |
| ----------------------------------- | ------------------------------------------------------------------------ |
| Is damaged                          | Moves it to `state.corrupt-<time>-<n>.json` and starts empty.            |
| Was written by a newer termx        | Copies it to `state.future-v<version>-<time>-<n>.json` and starts empty. |
| Was written by an older termx       | Copies it to `state.v<version>-<time>-<n>.json` and converts it.         |
| Has entries termx does not accept   | Copies it to `state.partial-<time>-<n>.json` and drops those entries.    |
| Cannot be read, or cannot be copied | Leaves it alone and saves nothing during this run.                       |

Each of these is recorded as a warning in the [log](logging.md). The five
most recent copies are kept.

To start from scratch, quit termx and delete `state.json`.
