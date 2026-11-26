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
    src/session.rs         Terminal session model
    src/config.rs          User configuration
apps/
  desktop/                 The desktop application
    src/                   Frontend (React, TypeScript, Vite)
      components/          UI components
      lib/ipc.ts           Typed wrappers around the Tauri commands
    src-tauri/             Tauri shell (Rust), depends on termx-core
      src/lib.rs           Application builder and command registration
      src/commands/        Tauri commands, one module per feature area
      capabilities/        Tauri permissions granted to the windows
      tauri.conf.json      Tauri configuration
plugin/                    Placeholder for the future Claude Code plugin
```

### Where things go

- **Core logic** goes in `crates/termx-core`. Add a module file under `src/`
  and declare it in `src/lib.rs`. Keep this crate free of Tauri and UI
  dependencies.
- **A new Tauri command** goes in a module under
  `apps/desktop/src-tauri/src/commands/`. Declare new modules in
  `commands/mod.rs`, register the command in the `generate_handler!` list in
  `src-tauri/src/lib.rs`, and add a typed wrapper in
  `apps/desktop/src/lib/ipc.ts`.
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

## Code style

- Code, comments, documentation and commit messages are written in English.
- Rust code is formatted with rustfmt and must pass clippy without warnings.
- TypeScript is compiled in strict mode, linted with ESLint and formatted
  with Prettier.
- New behaviour comes with tests: Rust unit tests next to the code, and
  Vitest tests next to the frontend modules (`*.test.ts` or `*.test.tsx`).

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
- Mark a breaking change with `!` before the colon.

Continuous integration checks the pull request title and every commit of the
pull request, so the history of `main` stays conventional whether a pull
request is squashed (the title becomes the commit message), rebased or merged.
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

If the last step fails after the commit was pushed, re-run the failed job
instead of starting the workflow again. Installers are not built or attached
to the release yet.

When you add a crate to the workspace, also add it to `versioned_files` in
[knope.toml](knope.toml) so that its `Cargo.lock` entry is bumped on release.

## License

By contributing, you agree that your contributions are licensed under the
[MIT License](LICENSE).
