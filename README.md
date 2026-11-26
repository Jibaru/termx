# termx

[![CI](https://github.com/Jibaru/termx/actions/workflows/ci.yml/badge.svg)](https://github.com/Jibaru/termx/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

termx is an open-source, cross-platform terminal emulator for Windows, Linux
and macOS, designed for running many [Claude Code](https://claude.com/claude-code)
sessions at once.

## Status

**Early development.** termx opens your login shell in a working terminal:
one session per window for now. There are no releases yet; see
[Build, run and test](#build-run-and-test) to try it.

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
- On macOS, **Option is the Meta key**, as Claude Code and shells expect for
  shortcuts such as Option+B or Option+Enter. It therefore does not type the
  alternate characters of your keyboard layout.
- Programs may copy to your clipboard (the OSC 52 escape sequence, used by
  Claude Code's `/copy` and by tmux). They can never read it.
- The scrollback keeps 10,000 lines.

Not there yet: the kitty keyboard protocol, font ligatures, inline images,
links to files, search, and settings (the font, colours and key bindings are
fixed for now).

## Planned features

- Many terminal sessions in one window, listed in a sidebar
- At-a-glance status of each Claude Code session (working, waiting, done)
- A companion Claude Code plugin that reports session state to termx
- Configurable shell, fonts, colours and key bindings
- Installers for Windows, Linux and macOS

## Architecture

termx is a [Tauri 2](https://v2.tauri.app) application.

| Path                     | What it is                                                     |
| ------------------------ | -------------------------------------------------------------- |
| `crates/termx-core`      | Rust library with the core logic. No UI or Tauri dependencies. |
| `apps/desktop/src-tauri` | Tauri shell that exposes the core to the webview.              |
| `apps/desktop/src`       | Frontend, written in React and TypeScript and built with Vite. |
| `plugin`                 | Placeholder for the future Claude Code plugin.                 |

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

Build the frontend, then the desktop binary:

```sh
pnpm build
cargo build -p termx-desktop
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

## Logs and bug reports

termx records errors and crashes in a log file on your machine. Nothing is
uploaded: there is no telemetry. To report a problem, click **Export
diagnostics** in the termx window and attach the zip file it saves to a
[bug report](https://github.com/Jibaru/termx/issues/new?template=bug_report.yml).

[Logging and diagnostics](docs/logging.md) explains where the logs are, what
they contain and what they never contain.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE)
