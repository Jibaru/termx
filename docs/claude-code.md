# Claude Code in termx

termx shows what each Claude Code session it hosts is doing: working,
waiting for your permission, waiting for your answer, or done. It learns
it from a small Claude Code plugin of its own, which it installs only if
you say yes. Without the plugin termx is a plain terminal for Claude Code.

## What installing changes

termx asks once and shows the list below before changing anything.

1. It writes the plugin into its state directory (see
   [What termx remembers](state.md)), in `claude-plugin/`: a manifest, a
   `hooks/hooks.json` and a copy of `termx-cli`, the program the hooks
   run.
2. It adds two entries to your Claude Code user settings,
   `~/.claude/settings.json` (or `$CLAUDE_CONFIG_DIR/settings.json`):

   ```json
   "extraKnownMarketplaces": {
     "termx": { "source": { "source": "directory", "path": "<state dir>/claude-plugin" } }
   },
   "enabledPlugins": { "termx@termx": true }
   ```

   These are the same entries `claude plugin marketplace add` and
   `claude plugin install` write. Everything else in the file, your own
   hooks included, stays exactly as it was, with its formatting. Before
   the first change termx copies the file to `settings.json.termx-backup`.
   A file that is not valid JSON is left alone and termx says so.

Claude Code picks the plugin up in the sessions it starts from then on.

## Removing it

Uninstall from termx removes those two entries (your file comes back byte
for byte), what Claude Code recorded about the plugin, and the plugin
directory. You can also turn the plugin off in Claude Code with `/plugin`.

## What the hooks send

Inside a terminal of termx, each hook sends termx: the name of the event,
the identifier of the conversation (so that Resume continues it), the type
of a notification, the source or reason of a session start or end, the
identifier of a subagent, the process number of Claude Code, and the tool
name only for `AskUserQuestion` and `ExitPlanMode`. Never your prompt, a
command, a file, a tool's output, a message, a path or the transcript.

They send it over a local socket that only your user can open (a Unix
socket with mode 0600, or a named pipe restricted to your user on Windows),
with a secret drawn for each run of termx. Nothing leaves your machine.

Outside termx (another terminal, CI, `claude -p`) the hooks find none of
termx's variables and exit at once, silently. They never block Claude
Code: all but `SessionEnd` run asynchronously, and the program gives up
after 300 ms.

## When termx does not know

termx never guesses from the screen. If a session said it was working and
then nothing is heard and nothing is printed for 60 seconds, or the Claude
Code process is gone without saying goodbye, the state becomes _unknown_.
It also resets when the session stops or restarts, and is never saved:
after a restart, sessions show no agent state until Claude Code reports
again.
