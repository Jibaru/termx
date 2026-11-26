# termx

[![CI](https://github.com/Jibaru/termx/actions/workflows/ci.yml/badge.svg)](https://github.com/Jibaru/termx/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

termx is an open-source, cross-platform terminal emulator for Windows, Linux
and macOS, designed for running many [Claude Code](https://claude.com/claude-code)
sessions at once.

## Status

**Early development.** The repository currently contains the project
skeleton: a desktop window with a placeholder UI wired to the Rust core. It is
not a usable terminal yet, and there are no releases.

## Planned features

- Many terminal sessions in one window, listed in a sidebar
- A fast terminal view backed by a native pseudo-terminal on every platform
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
