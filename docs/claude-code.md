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
   `claude plugin install` write. The Claude Code documentation describes
   `extraKnownMarketplaces` with a `directory` source for project settings;
   that Claude Code also picks it up from your user settings rests on a
   check with Claude Code 2.1.321, not on the documentation. Everything else in the file, your own
   hooks included, stays exactly as it was, with its formatting. Before
   the first change termx copies the file to `settings.json.termx-backup`.
   A file that is not valid JSON, a read-only file, or a link to a file
   that does not exist is left alone and termx says so.

   If your settings already have a marketplace named `termx` of your own,
   or you turned the plugin off (`"termx@termx": false`), termx installs
   nothing and shows a conflict: that is your choice to make.

Claude Code picks the plugin up in the sessions it starts from then on.

## Removing it

termx records what your settings looked like before it installed, in
`claude-plugin-install.json` in its state directory (readable by you
only). Uninstall from termx then:

- gives the file back byte for byte when you have not changed it since,
  or removes it if termx created it;
- otherwise removes only its two entries, and the objects around them only
  if termx created those;
- never touches a marketplace named `termx` that is not its own, or a
  `false` you set;
- removes what Claude Code recorded about the plugin, only when Claude
  Code recorded it for termx's directory, and the plugin directory.

You can also turn the plugin off in Claude Code with `/plugin`.

## When the hooks cannot run

Organisations can set `allowManagedHooksOnly` (or `disableAllHooks`) in
the managed settings of Claude Code, and you can set `disableAllHooks` in
yours. Claude Code then silently skips the hooks of the plugin. termx
reads those settings and shows the plugin as _blocked_ instead of
pretending it works.

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
after 250 ms reading its input and 300 ms waiting for termx.

## When termx does not know

While Claude asks for permission, the row says so until the tool runs,
the turn ends, or you answer in the terminal (Enter, a number, or Escape).

termx never guesses from the screen. If a session said it was working and
then nothing is heard and nothing is printed for 60 seconds, or the Claude
Code process is gone without saying goodbye, the state becomes _unknown_.
It also resets when the session stops or restarts, and is never saved:
after a restart, sessions show no agent state until Claude Code reports
again.
