# termx plugin for Claude Code

This plugin tells [termx](https://github.com/Jibaru/termx) what each Claude
Code session it hosts is doing, so that its sidebar can show it: working,
waiting for your permission, waiting for your answer, done.

termx installs it itself, only after asking you, and can remove it again.
You do not install it by hand.

## What it does

It registers one hook for each of these events of Claude Code:
`SessionStart`, `SessionEnd`, `UserPromptSubmit`, `PreToolUse`,
`PostToolUse`, `PostToolUseFailure`, `PermissionRequest`,
`PermissionDenied`, `Notification`, `Stop`, `StopFailure`, `SubagentStart`,
`SubagentStop`, `Elicitation` and `ElicitationResult`. Each runs
`termx-cli hook`, a small program shipped with termx, without a shell.

- **Outside termx** (another terminal, CI, `claude -p`), the program sees
  none of the variables termx gives its terminals and exits at once,
  without reading anything, printing anything or failing.
- **Inside termx**, it sends termx the name of the event, the identifier of
  the conversation, the notification type, the source or reason of a
  session start or end, whether it ran for a subagent, the process of Claude
  Code, and the name of the tool only for `AskUserQuestion` and
  `ExitPlanMode`. Never the prompt, the input or output of a tool, a
  message, a path or the transcript. It goes over a socket that only your
  user can open, and nowhere else: nothing leaves your machine.

Every hook but `SessionEnd` is asynchronous, so Claude Code never waits for
it, and the program gives up on termx after 300 ms.

## The files here

`.claude-plugin/plugin.json` and `hooks/hooks.json` are templates: termx
fills in its version (`@VERSION@`) and the path of its copy of
`termx-cli` (`@TERMX_CLI@`) when it writes the plugin on your machine. See
`crates/termx-core/src/claude.rs` for where it goes and what changes in
your Claude Code settings.
