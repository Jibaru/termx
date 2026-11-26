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

> **Placeholder.** This section will be filled in by a later change that
> introduces the changelog and release tooling.

Until then, write commit messages following
[Conventional Commits](https://www.conventionalcommits.org) (for example
`feat: ...`, `fix: ...`, `docs: ...`, `chore: ...`).

## License

By contributing, you agree that your contributions are licensed under the
[MIT License](LICENSE).
