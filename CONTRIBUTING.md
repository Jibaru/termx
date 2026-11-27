# Contributing to termx

Thanks for your interest in termx. The project is in early development, so
expect things to move quickly.

## Development setup

1. Install the prerequisites for your platform listed in the
   [README](README.md#prerequisites).
2. Clone the repository and install the JavaScript dependencies:

   ```sh
   pnpm install
   ```

3. Start the app in development mode:

   ```sh
   pnpm dev
   ```

## Repository layout

```text
Cargo.toml                 Cargo workspace root: shared package metadata and lints
knope.toml                 Changelog and release configuration
.changeset/                Change files waiting for the next release
crates/
  termx-core/              Core library. Must not depend on Tauri or any UI code
    src/lib.rs             Public API and module declarations
    src/pty.rs             Pseudo-terminal handling
    src/session.rs         Terminal sessions: registry, output batching, flow control
    src/workspace.rs       Sessions as the user sees them: model, projects and Git,
                           launching, layout, state file, change notifications
    src/config.rs          User configuration
    src/diagnostics/       Logging, panic capture, redaction, diagnostics bundle
apps/
  desktop/                 The desktop application
    src/                   Frontend (React, TypeScript, Vite)
      components/          UI components
      terminal/            The xterm.js terminal: controller, key map, look, session store
      lib/ipc.ts           Typed wrappers around the Tauri commands, and their types
      lib/ipc.contract.json  Examples of what the two sides exchange (generated)
    src-tauri/             Tauri shell (Rust), depends on termx-core
      src/lib.rs           Application builder and command registration
      src/commands/        Tauri commands, one module per feature area
      src/logging.rs       Log directory per platform, logging start and stop
      src/e2e.rs           Hooks for the end-to-end tests, only in builds made for them
      src/terminal.rs      Terminals of the app: channels, window ownership, shutdown
      src/workspace.rs     Workspace of the app: state directory, change events, window
      src/contract.rs      Test that keeps lib/ipc.contract.json true to the Rust types
      capabilities/        Tauri permissions granted to the windows
      tauri.conf.json      Tauri configuration
e2e/                       End-to-end tests, which drive the real application
  specs/                   The tests, one file per area
  src/                     The harness: starting the application, typing, reading the terminal
  helpers/                 The programs the tests run inside the terminal
docs/                      User documentation
plugin/                    Placeholder for the future Claude Code plugin
```

### Where things go

- **Core logic** goes in `crates/termx-core`. Add a module file under `src/`
  and declare it in `src/lib.rs`. Keep this crate free of Tauri and UI
  dependencies.
- **A new Tauri command** goes in a module under
  `apps/desktop/src-tauri/src/commands/`. Declare new modules in
  `commands/mod.rs`, register the command in the `command_handlers!` list in
  `src-tauri/src/lib.rs`, and add a typed wrapper in
  `apps/desktop/src/lib/ipc.ts`. A command that can fail follows the pattern
  in [Logging conventions](#logging-conventions). Call it in
  `src-tauri/src/ipc_tests.rs` with the arguments the wrapper sends: that
  test goes through Tauri's IPC without opening a window, and is what
  catches an argument that is named differently on the two sides.
- **A type sent to or received from the frontend** is declared on both
  sides: in Rust, and in `lib/ipc.ts`. `lib/ipc.contract.json` holds an
  example of each, and a test on each side checks its types against it, so
  a change made on one side only fails. After changing such a type, update
  `lib/ipc.ts` and `ipc.contract.test.ts`, then write the examples again
  with `TERMX_UPDATE_CONTRACT=1 cargo test -p termx-desktop contract`.
- **A new crate** goes in `crates/`. It is picked up by the workspace
  automatically. Inherit the shared metadata and lints with
  `version.workspace = true` (and so on) and `[lints] workspace = true`.

## Checks

Continuous integration runs the following on Linux, macOS and Windows. Run
them locally before opening a pull request:

```sh
pnpm build                                              # the Tauri crate embeds the frontend
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm lint
pnpm typecheck
pnpm test
```

`pnpm format` formats everything Prettier covers, and `cargo fmt --all`
formats the Rust code.

Continuous integration also runs the [end-to-end tests](#end-to-end-tests) on
the three platforms.

## End-to-end tests

The tests in `e2e/` start the real application and use it: a real webview,
the real xterm.js, a real shell in a real PTY. They catch what the unit tests
cannot, because those replace xterm.js and the Rust side with stand-ins: keys
that send the wrong bytes, a terminal that stays blank, a resize that never
reaches the program, a shell that survives the application.

### Running them

```sh
pnpm e2e:build   # the frontend, then a debug build with the `e2e` feature
pnpm e2e         # every test
pnpm e2e specs/keys.e2e.ts          # one file
pnpm e2e -t "Shift+Enter"           # the tests whose name matches
```

`pnpm e2e:build` again after changing the application; the tests themselves
need no build.

- **macOS**: nothing else to install.
- **Windows**: nothing else to install. The WebView2 runtime is part of
  Windows 10 and 11. `pnpm e2e` goes through Windows PowerShell once, to give
  the suite the default handling of Ctrl+C whatever started it (see
  `e2e/run.mjs`).
- **Linux**: a display is needed. On a desktop, run the commands as they
  are. Without one (a server, a container), run the suite under a virtual
  display: install `xvfb` and use
  `xvfb-run --auto-servernum --server-args="-screen 0 1920x1080x24" pnpm e2e`.
  CI does that, and also takes a picture of that display with `import`, from
  ImageMagick, to check that the terminal is drawn on it; outside CI that one
  check is left out and ImageMagick is not needed.

Each test file opens a window of termx for a few seconds, about a minute in
all. **The window takes the keyboard focus**: what you type meanwhile lands
in the terminal under test, which loses you those keys and can fail the test.
Leave the keyboard alone while the suite runs.

The tests never touch your files or your shell configuration: every
application they start gets an empty home directory, which also holds its
state directory (`TERMX_STATE_DIR`) and, on Windows, the profile of WebView2
(`WEBVIEW2_USER_DATA_FOLDER`), its own log directory, none of the `TERMX_*`
variables of your environment, and a shell that reads no profile
(`bash --noprofile --norc`, or `cmd.exe /D` on Windows). The home directory
is deleted afterwards, also when the application failed to start. One test overwrites the system clipboard; it only
runs in CI, or with `TERMX_E2E_CLIPBOARD=1`.

When a test fails, look in `e2e/artifacts/<name of the run>/`: `failures/`
has what the terminal showed, its state and a screenshot for each failed
test; `app-output.log` and `logs/` have what the application printed and
logged. CI uploads the same directory as an artifact of the failed job.

### How they work

`pnpm e2e:build` compiles the application with the cargo feature `e2e`
(`apps/desktop/src-tauri/src/e2e.rs`). That feature, and nothing else, adds:

- a WebDriver server inside the application, the
  [embedded server](https://webdriver.io/docs/desktop-testing/tauri) of the
  WebdriverIO Tauri service. It is used on the three platforms so that there
  is one harness: macOS has no WebDriver for its webview, which rules out
  `tauri-driver` there. It only listens when `TAURI_WEBDRIVER_PORT` is set,
  and only on the loopback interface;
- `TERMX_E2E_SHELL`, which names the program the terminals run instead of
  your login shell;
- a marker that makes the frontend expose its terminals, so that a test can
  read the buffer of xterm.js (`apps/desktop/src/test/e2eHook.ts`).

None of this can ship. The feature is off by default, and it is a compile
error in a build without debug assertions, which a release is, in any
profile that inherits from `release`, and in any build that does not set
`TERMX_ALLOW_E2E=1`, which only `pnpm e2e:build` (`e2e/build.mjs`) and the
End-to-end workflow do; that workflow checks the three refusals. Without
the marker the frontend exposes nothing. The capabilities and the
content security policy of the application are the same with and without the
feature. Keep it that way: a hook for the tests goes behind the feature, in
`e2e.rs`.

**Keys are not physical key presses.** No tool can press keys in the webview
of the three platforms, so the suite delivers `KeyboardEvent`s to the
terminal, described as the webview describes the real key (`key`, `code`,
`keyCode`, which of the two Option keys). That tests everything from the key
handler of termx to the program in the terminal, and not the webview turning
a physical key into that event. Pastes are `paste` events, for the same
reason.

### Writing one

A test file starts one application for all its tests with `useApp`, and
uses its terminal:

```ts
import { describe, it } from "vitest";

import { useApp } from "../src/fixture.ts";

describe("typing in the terminal", () => {
  const fixture = useApp("typing"); // also the name of the artifacts directory

  it("runs a typed command and shows its output", async () => {
    const { terminal } = fixture;
    const token = terminal.token();

    await terminal.submit(`echo ${token}`); // types it, then Enter

    await terminal.waitForExactLine(token); // read from the buffer of xterm.js
  });
});
```

The rules that keep the suite deterministic:

- **Wait on a condition, never for a fixed time.** `waitForExactLine`,
  `waitForLine`, `waitForState` and `waitFor` check until the condition holds
  or the time is up, and say what the terminal showed when it is.
- **Never repeat a test.** A test that fails once in a while is a failing
  test: find out why. The only thing that is repeated is a start of the
  application whose WebDriver server lost its port to another process, at
  most once per run of the suite; a second one fails the test, and CI
  annotates every one. An application that exits while it starts is a
  failure, never repeated, with what it printed attached. When a terminal
  does not get to its prompt, the failure also describes the page
  (`startup-diagnostics.json` in the directory of the run): visibility,
  focus, status of the session, what the buffer of xterm.js holds.
- **Tell your output apart with a token.** The tests of a file share a
  terminal, and the command line you type is on screen too. Wait for a line
  that _is_ `terminal.token()`, not for one that contains a fixed word.
- **Type what both shells understand.** The same test runs under bash and
  under `cmd.exe`: `echo`, `exit 3` and little else. For anything more, add a
  command to `e2e/helpers/helper.mjs` and start it with
  `terminal.startHelper`. `terminal.startRaw()` starts the one that prints
  the bytes it receives, which is how keys and pastes are checked.
- **Assert on text and state, not on pixels.** `terminal.lines()` and
  `terminal.state()` read xterm.js. The one test that looks at pixels
  (`specs/launch.e2e.ts`) only counts those that are not the background,
  with a tolerance, and compares no image.
- Code that runs in the page lives in `e2e/src/page.ts`. It is sent to the
  webview as text, so it cannot use anything from outside its function.
- A test that only makes sense on one platform says so:
  `it.runIf(process.platform === "darwin")`.

If a test shows a bug in the application that is not fixed in the same pull
request, keep the test and mark it `it.fails`, with a comment that says what
is wrong and links to where it is explained. Do not skip it and do not weaken
it: the test starts failing when the bug is fixed, which is the reminder to
remove the mark. `specs/ctrl-c.e2e.ts` has one.

## Code style

- Code, comments, documentation and commit messages are written in English.
- Rust code is formatted with rustfmt and must pass clippy without warnings.
- TypeScript is compiled in strict mode, linted with ESLint and formatted
  with Prettier.
- New behaviour comes with tests: Rust unit tests next to the code, and
  Vitest tests next to the frontend modules (`*.test.ts` or `*.test.tsx`).
  Behaviour that only shows in the running application (keys, rendering, the
  life of the shell) also gets an [end-to-end test](#end-to-end-tests).

## Logging conventions

termx writes a log file on the user's machine, which users attach to bug
reports. [docs/logging.md](docs/logging.md) describes it from the user's side;
this section is what to do in the code.

### What must never be logged

The log is shared in public issues. These rules have no exceptions, at any
level, including `debug` and `trace`:

- **No terminal output**: nothing read from a PTY, in whole or in part.
- **No keystrokes or input**: nothing written to a PTY, no clipboard content.
- **No environment variable values.** Names are fine.
- **No file contents.**
- **No command lines**: not the command of a background session, not the
  arguments of any program. The path of the program is fine.
- **No user or machine identifiers**: user name, host name, addresses.

Log sizes, counts, identifiers, durations, exit codes and error kinds instead:
`bytes = chunk.len()`, never `chunk`.

Nothing in the logger can recognise such data, so these rules are enforced by
review. Treat a logging call that takes a value from a PTY, from the
environment or from a file as a bug.

What the logger does on its own, to every message and text field:

- It replaces the home directory with `~` and the name in any other
  home-shaped path (`/Users/x`, `/home/x`, `C:\Users\x`) with `<user>`. So
  paths are safe to log.
- Best effort, it replaces the bare user name with `<user>` and anything
  shaped like a token, a password or a key with `<redacted>`. Do not rely on
  it: it is the last line of defence, not a licence to log request headers.

For a path that leaves the application some other way (an error returned to
the interface, a file you write), use `termx_core::diagnostics::redact::path`.
Keep [docs/logging.md](docs/logging.md#privacy) accurate when you change any of
this: it tells users exactly what is guaranteed and what is best effort.

### In Rust

Use the `tracing` macros, re-exported as `termx_core::diagnostics::tracing`.
Do not use `println!`, `eprintln!` or the `log` crate. Put details in fields
instead of formatting them into the message, and keep the message a constant,
lower-case phrase:

```rust
use termx_core::diagnostics::tracing::{debug, error, info, warn};

info!(session_id, shell = %shell.display(), "session started");
error!(session_id, error = %source, "could not resize the terminal");
```

| Macro    | Use it for                                                               |
| -------- | ------------------------------------------------------------------------ |
| `error!` | An operation failed and the user is affected.                            |
| `warn!`  | Something unexpected that termx recovered from, such as a fallback.      |
| `info!`  | Lifecycle events that happen a few times per run: start, session opened. |
| `debug!` | Details for investigating a problem. Off by default.                     |
| `trace!` | Very frequent events, such as one per chunk read. Off by default.        |

Log an error once, where it is handled, not at every level it passes through.
Panics are recorded automatically by the panic hook.

`warn!` and `error!` return once the record is written (or after 100 ms if the
disk does not answer), so that the record survives a crash right after it.
That costs tens of microseconds: fine for things that go wrong, not for a hot
loop. Repeats from the same line with the same message, numbers aside, are
written 5 times per 10 seconds and then counted, so put what varies in fields
or numbers and keep the words of the message constant. `info!` and lower are
queued and cost almost nothing, but can be dropped under extreme load.

To make a panic record say what was going on, run the code inside
`termx_core::diagnostics::with_panic_context("key", value, || ...)`.

A Tauri command that can fail or panic returns `CommandResult` and runs its
body inside `logged` (`logged_async` for an `async fn`). The wrapper writes one
`ERROR` record with the command name, gives the frontend a `CommandError`, and
turns a panic into an error (the panic record then names the command):

```rust
use super::error::{CommandResult, logged};

#[tauri::command]
pub fn rename_session(id: u32, title: String) -> CommandResult<()> {
    logged("rename_session", || sessions::rename(id, &title))
}
```

The `CommandError` the frontend receives has a `kind` it can act on
(`notFound`, `folderMissing`, `sessionRunning`, ...; see `ErrorKind` in
`commands/error.rs`). It comes from the error the body returns: the error
types of `termx-core` choose theirs, a plain message is `internal`, and a
body that knows better returns `Failure::new(ErrorKind::..., message)`.
Add a kind to `ErrorKind`, to `CommandErrorKind` in `lib/ipc.ts` and to the
contract together.

A panic must never escape a command: in a synchronous command it unwinds into
the platform's event loop and can abort the application, and in an
asynchronous one it leaves the promise in the frontend pending forever. Only a
command whose body cannot fail or panic may skip the wrapper.

Commands must not take a file system path to write to from the frontend. The
webview is less trusted than the Rust side: choose the location in Rust.

Run the app with `TERMX_LOG=debug` to see the more verbose levels. Debug
builds also print the records to stderr. In tests, build a logger with
`Diagnostics::build` and run the code under `tracing::dispatcher::with_default`
to assert on what was written.

### In the frontend

- Call commands through the wrappers in `src/lib/ipc.ts`. They use `call`,
  which logs a failed call once.
- Uncaught errors, unhandled rejections and rendering errors are logged
  automatically. For an error you catch and handle yourself, call
  `reportError("where-it-happened", error)` from `src/lib/errorReporting.ts`
  if it is worth knowing about.
- Throw `Error` objects. For anything else that is thrown or rejected, only
  the type, the number of keys and the names of plainly field-like keys are
  logged, never a value
  (`src/lib/logSafe.ts`). Put what must be known in the message of the error,
  and nothing that must not be logged.
- `console.log` and friends do not reach the log file.
- The same privacy rules apply to everything passed to these functions.

## Changelog and commits

termx has one version for the whole repository. The changelog and the version
bumps are produced by [Knope](https://knope.tech) from small Markdown _change
files_ that are added together with each change.

### Commit messages and pull request titles

Commit messages and pull request titles follow
[Conventional Commits](https://www.conventionalcommits.org) and are written in
English:

```text
<type>(<optional scope>): <description>
```

```text
feat(desktop): add a sidebar listing the open sessions
fix(core): keep the shell alive when the window is resized
docs: explain how to build on Fedora
refactor(core)!: rename the session identifier type
```

- The allowed types are `feat`, `fix`, `perf`, `docs`, `refactor`, `test`,
  `build`, `ci`, `chore`, `style` and `revert`.
- The scope is optional. Use the part of the project the change touches, such
  as `core`, `desktop` or `plugin`.
- Start the description in lower case and do not end it with a full stop.
- Keep the whole first line at 100 characters or fewer.
- Mark a breaking change with `!` before the colon.

Continuous integration checks the pull request title and every commit of the
pull request against these rules, so the history of `main` stays conventional
whether a pull request is squashed (the title becomes the commit message),
rebased or merged.

To revert a change, `git revert` is fine: its default `Revert "..."` commit
message is accepted. The pull request title is not, so change the title that
GitHub suggests to the `revert` type followed by the description of the
reverted change, for example `revert: add a sidebar listing the open sessions`.

To check your commits locally before pushing:

```sh
pnpm dlx --package @commitlint/cli --package @commitlint/config-conventional commitlint --from origin/main
```

### Change files

A pull request that changes what users see adds a change file in
`.changeset/`. Install Knope once, using one of the options from its
[installation guide](https://knope.tech/installation/):

```sh
brew install knope-dev/tap/knope   # macOS and Linux
choco install knope                # Windows
cargo install knope --locked       # any platform, builds from source
```

Then, from the repository root, run:

```sh
knope document-change
```

(`pnpm change` does the same.) Knope asks for the kind of change and a short
summary, and writes a file such as `.changeset/add_session_sidebar.md`:

```markdown
---
default: minor
---

# Add a session sidebar

The open sessions are now listed in a sidebar. Click a session to switch to
it.
```

- The value after `default:` is `major` for a breaking change, `minor` for a
  new feature and `patch` for a fix. `default` is the name Knope gives to the
  only package of this repository.
- The heading is the changelog entry. Write it for users, not for developers.
- Everything below the heading is optional detail. Edit the file freely, or
  write it by hand instead of using the command.
- Commit the file with the change it describes. Add more than one file when a
  pull request contains several independent changes.
- `.changeset/` holds change files only. Continuous integration rejects any
  other file there, and any change file without this front matter and a
  heading.
- Do not delete change files: the release does that. The exception is a
  `revert` pull request, which deletes the change file of the change it
  reverts if that change has not been released yet. If it has been released,
  add a new change file describing the revert instead.

While the version is below 1.0.0, Knope bumps one position lower than the
change type says: breaking changes bump the minor version, and features and
fixes bump the patch version.

### When a change file is not needed

Changes that users cannot notice need no change file: documentation, tests,
refactors, continuous integration, build tooling and other chores.

Continuous integration decides from the type in the pull request title:

| Pull request title                                               | Change file  |
| ---------------------------------------------------------------- | ------------ |
| `feat`, `fix`, `perf`, or any type marked as breaking with `!`   | Required     |
| `docs`, `refactor`, `test`, `build`, `ci`, `chore`, `style`, ... | Not required |

If a `feat`, `fix` or `perf` pull request really has nothing to tell users, a
maintainer can add the `no-changelog` label to skip the check. Adding a change
file to any other type of pull request is always allowed.

### Cutting a release

Releases are cut by a maintainer with the
[Release workflow](.github/workflows/release.yml), which only runs when
started by hand:

1. Preview the release locally with `pnpm release:dry-run` (or
   `knope prepare-release --dry-run`), or run the workflow with **dry-run**
   ticked. Nothing is changed.
2. Start the workflow on `main` from the Actions tab, or with:

   ```sh
   gh workflow run release.yml
   gh workflow run release.yml -f version=0.1.0   # choose the version yourself
   ```

The workflow then:

1. Moves the contents of the change files into a new section of
   [CHANGELOG.md](CHANGELOG.md) and deletes them.
2. Sets the new version in `Cargo.toml`, `Cargo.lock`, `package.json` and
   `apps/desktop/package.json`. Tauri reads the application version from
   `Cargo.toml`.
3. Pushes that as a `chore: release <version>` commit to `main`.
4. Tags the commit `v<version>` and publishes a GitHub Release with the new
   changelog section as its notes.

Installers are not built or attached to the release yet.

Things to know:

- **Nothing to release.** The workflow stops with an error when `.changeset/`
  has no change files, also when a version is given.
- **Recovering from a failed run.** If the workflow fails after the release
  commit was pushed (step 3) but before the tag and the GitHub Release exist,
  start the workflow again or re-run the failed job. A new run notices that
  the version in `Cargo.toml` has a changelog section but no tag, skips
  steps 1 to 3, and tags the original release commit, even if other pull
  requests were merged in the meantime. Their change files stay for the next
  release.
- **The release commit is not checked by CI.** It is pushed with the
  workflow's `GITHUB_TOKEN`, and GitHub does not start workflows for such
  pushes. The commit only changes versions and the changelog, and the workflow
  verifies `Cargo.lock` with `cargo metadata --locked` before pushing.
- **Branch protection.** The workflow pushes directly to `main`. If `main` is
  later protected against direct pushes, the push will be rejected: the
  workflow then has to push through an identity that is allowed to bypass the
  protection, or open a release pull request instead.

When you add a crate to the workspace, also add it to `versioned_files` in
[knope.toml](knope.toml) so that its `Cargo.lock` entry is bumped on release.

## License

By contributing, you agree that your contributions are licensed under the
[MIT License](LICENSE).
