# termx

[![CI](https://github.com/Jibaru/termx/actions/workflows/ci.yml/badge.svg)](https://github.com/Jibaru/termx/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

termx is an open-source, cross-platform terminal emulator for Windows, Linux
and macOS, designed for running many [Claude Code](https://claude.com/claude-code)
sessions at once.

## Status

**Early development.** termx runs many sessions (Claude Code, shells and
background commands) in one window, listed in a sidebar grouped by project,
and shown in panes you can split. It remembers them and its window between
runs ([what termx remembers](docs/state.md)). There are no releases yet; see
[Build, run and test](#build-run-and-test) to try it.

## Sessions

The sidebar lists every session, grouped by project: a Git repository and
its worktrees form one group, a plain folder its own. Background commands
have a section of their own. Each row shows the title (the name you gave it,
else the title its program set), the kind, the Git branch, whether it runs,
and how long ago it last printed something. Drag rows and groups to reorder
them, or use Alt+Up and Alt+Down; double-click a row, or press F2, to rename
it (an empty name gives the automatic title back). Right-click a row for
everything else.

When termx starts again, your sessions come back without running. Press
Enter in a pane, click Resume, or Resume all in the banner: Claude Code
carries on with its conversation, a shell or a command starts again in the
same folder.

| Action                                 | macOS             | Windows and Linux  |
| -------------------------------------- | ----------------- | ------------------ |
| New session                            | Shift+Cmd+T       | Ctrl+Shift+T       |
| New Claude session in the same project | Shift+Cmd+N       | Ctrl+Shift+N       |
| Close session (asks if it runs)        | Shift+Cmd+W       | Ctrl+Shift+W       |
| Split right                            | Cmd+D             | Ctrl+Shift+D       |
| Split down                             | Shift+Cmd+D       | Ctrl+Shift+E       |
| Close pane (the session keeps running) | Cmd+W             | Ctrl+Shift+Alt+W   |
| Focus the pane in a direction          | Option+Cmd+arrow  | Ctrl+Shift+arrow   |
| Next and previous session              | Shift+Cmd+] and [ | Ctrl+Shift+] and [ |
| Go to session 1 to 9                   | Cmd+1 to 9        | Ctrl+Shift+1 to 9  |
| Show or hide the sidebar               | Cmd+B             | Ctrl+Shift+B       |
| Go to the sidebar                      | Cmd+0             | Ctrl+Shift+0       |
| Go to the session waiting longest      | Shift+Cmd+A       | Ctrl+Shift+A       |
| Rename the session (in the sidebar)    | F2                | F2                 |

On macOS the same actions are in the menu bar. Cmd+W closes the focused
pane and never the window; with one pane it does nothing. Closing the window
or quitting while sessions run asks first: they stop, and come back, not
running, the next time termx starts.

These never take a key a program in the terminal needs: Ctrl and Alt
shortcuts (Ctrl+C, Ctrl+R, Ctrl+B...), Escape, Shift+Tab and the function
keys all reach the terminal. That is why Windows and Linux use Ctrl+Shift,
and why F2 renames only while the sidebar has the keyboard.

## Claude Code

Each row of a Claude Code session can show what Claude is doing: working,
waiting for your permission or your answer, or done. termx learns it from
a Claude Code plugin of its own, installed only if you agree; see
[Claude Code in termx](docs/claude-code.md) for exactly what that changes
and what the plugin sends.

### Which session needs you

termx can show what the agent of each Claude Code session is doing: working,
waiting for your permission, waiting for your answer, or done. For that it
installs a small plugin in Claude Code, once you agree: the first time termx
finds Claude Code it asks, with the exact list of files the plugin adds.
Your own hooks and settings are not changed, and **Remove**, at the bottom of
the sidebar, takes the plugin out again. If you said not now, **Enable Claude
Code status** in the same place asks again.

- A session that waits for you is marked on its row (a magenta bar and
  icon, a bold title) and in the header of its pane. A collapsed project
  says how many of its sessions wait.
- **Needs you**, at the top of the sidebar, lists them, the one that has
  waited longest first. Shift+Cmd+A (Ctrl+Shift+A on Windows and Linux)
  goes to that one; pressed again, to the next. **Next Session That Needs
  You**, in the View menu, goes through them in the order of the sidebar.
- When a session starts to wait and you are not looking at it (the window
  is in the background, or no pane shows it), termx sends one notification,
  after a couple of seconds, so that a permission granted at once does not
  notify. Several at once are one notification. Click it to go to the
  session. The icon of termx shows how many sessions wait. "Notify me when a
  session needs me", at the bottom of the sidebar, turns notifications off;
  "Also when a session is done" turns them on for finished sessions.
- A session that stopped reporting is shown dimmed, with when it last
  reported, and never as waiting.

## Using the terminal

| Action                         | macOS       | Windows and Linux            |
| ------------------------------ | ----------- | ---------------------------- |
| Copy the selection             | Cmd+C       | Ctrl+Shift+C or Ctrl+Insert  |
| Paste                          | Cmd+V       | Ctrl+Shift+V or Shift+Insert |
| Open a link in the browser     | Cmd+click   | Ctrl+click                   |
| New line in Claude Code        | Shift+Enter | Shift+Enter                  |
| Restart after the shell exited | Enter       | Enter                        |

- **Shift+Enter** sends the same sequence as Option+Enter (`ESC` followed by
  Enter), which Claude Code reads as "new line" without any setup. Do not run
  `/terminal-setup` for it.
- On macOS, the **left Option key is the Meta key**, as Claude Code and
  shells expect for shortcuts such as Option+B. The **right Option key types
  characters** as usual: á, ñ, ¿ and, on keyboards that need Option for them,
  @ # [ ] { } \ | ~. Option+Enter inserts a new line in Claude Code with
  either key.
- Programs may copy to your clipboard (the OSC 52 escape sequence, used by
  Claude Code's `/copy` and by tmux). They can never read it.
- The scrollback keeps 10,000 lines.

Not there yet: the kitty keyboard protocol, font ligatures, inline images,
links to files, search, and settings (the font, colours and key bindings are
fixed for now).

## Planned features

- Configurable shell, fonts, colours and key bindings
- Installers for Windows, Linux and macOS

## Architecture

termx is a [Tauri 2](https://v2.tauri.app) application.

| Path                     | What it is                                                     |
| ------------------------ | -------------------------------------------------------------- |
| `crates/termx-core`      | Rust library with the core logic. No UI or Tauri dependencies. |
| `apps/desktop/src-tauri` | Tauri shell that exposes the core to the webview.              |
| `apps/desktop/src`       | Frontend, written in React and TypeScript and built with Vite. |
| `e2e`                    | End-to-end tests, which drive the real application.            |
| `plugin`                 | The Claude Code plugin termx installs, as a template.          |
| `crates/termx-protocol`  | The wire protocol of the local socket of termx.                |
| `crates/termx-cli`       | `termx-cli`, the hook sender the plugin runs.                  |

## Prerequisites

All platforms need:

- [Rust](https://rustup.rs) (the stable toolchain is selected automatically by
  `rust-toolchain.toml`)
- [Node.js](https://nodejs.org) 22 or newer
- [pnpm](https://pnpm.io) 11

Each platform also needs the Tauri system dependencies. The
[Tauri prerequisites guide](https://v2.tauri.app/start/prerequisites/) is the
authoritative reference; the short version is below.

### Linux

On Debian and Ubuntu:

```sh
sudo apt update
sudo apt install libwebkit2gtk-4.1-dev \
  build-essential \
  curl \
  wget \
  file \
  libxdo-dev \
  libssl-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev
```

For other distributions, see the Tauri prerequisites guide.

### macOS

Install the Xcode Command Line Tools:

```sh
xcode-select --install
```

### Windows

- [Microsoft C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
  with the "Desktop development with C++" workload
- [Microsoft Edge WebView2](https://developer.microsoft.com/microsoft-edge/webview2/),
  which is already included in Windows 10 (version 1803 and later) and
  Windows 11

## Build, run and test

Install the JavaScript dependencies once:

```sh
pnpm install
```

Run the app in development mode, with hot reload for the frontend:

```sh
pnpm dev
```

Build the frontend, then the desktop binary and the hook sender of its
Claude Code plugin (`termx-cli`, which termx looks for next to itself;
`pnpm dev` builds it too):

```sh
pnpm build
cargo build -p termx-desktop -p termx-cli
```

The Tauri crate embeds the frontend, so `apps/desktop/dist` must exist before
any `cargo` command that compiles `termx-desktop`. Run `pnpm build` first.

Build a bundled application for the current platform:

```sh
pnpm tauri build
```

Run the checks:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm lint
pnpm typecheck
pnpm test
```

Run the end-to-end tests, which open the real application (see
[CONTRIBUTING.md](CONTRIBUTING.md#end-to-end-tests)):

```sh
pnpm e2e:build
pnpm e2e
```

## Logs and bug reports

termx records errors and crashes in a log file on your machine. Nothing is
uploaded: there is no telemetry. To report a problem, click **Export
diagnostics** in the termx window and attach the zip file it saves to a
[bug report](https://github.com/Jibaru/termx/issues/new?template=bug_report.yml).

[Logging and diagnostics](docs/logging.md) explains where the logs are, what
they contain and what they never contain.

## What termx remembers

termx keeps its sessions, their layout and its window in a state file on
your machine, and nothing that was printed or typed in a terminal.
[What termx remembers](docs/state.md) says where the file is, what is in it
and what happens when two termx run at once.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE)
