#!/usr/bin/env bash
# Prints the value of CARGO_ENCODED_RUSTFLAGS for release builds.
#
# rustc embeds absolute source paths in a binary (panic locations, debug
# info), and they show up in the backtraces termx writes to its log. Built
# as is, a release would carry the home directory of whoever built it into
# every user's log. `--remap-path-prefix` replaces those prefixes at compile
# time:
#
#   the repository            ->  .
#   the Cargo home            ->  /cargo
#   the rustup home           ->  /rustup
#   the home directory        ->  ~
#
# Usage, in the job that builds the installers:
#
#   export CARGO_ENCODED_RUSTFLAGS="$(.github/scripts/release-rustflags.sh)"
#   pnpm tauri build
#
# CARGO_ENCODED_RUSTFLAGS separates flags with the 0x1f character instead of
# spaces, so paths that contain spaces work. It takes precedence over
# RUSTFLAGS. See docs/logging.md, "Privacy".
set -euo pipefail

# rustc sees native paths. On Windows, Git Bash has to convert its own.
native() {
  if command -v cygpath >/dev/null 2>&1; then
    cygpath -w "$1"
  else
    printf '%s' "$1"
  fi
}

root="$(native "$(git rev-parse --show-toplevel)")"
cargo_home="$(native "${CARGO_HOME:-$HOME/.cargo}")"
rustup_home="$(native "${RUSTUP_HOME:-$HOME/.rustup}")"
home="$(native "$HOME")"

# More specific prefixes come last: rustc applies the last one that matches.
flags=(
  "--remap-path-prefix=$home=~"
  "--remap-path-prefix=$rustup_home=/rustup"
  "--remap-path-prefix=$cargo_home=/cargo"
  "--remap-path-prefix=$root=."
)

separator=$'\x1f'
joined=""
for flag in "${flags[@]}"; do
  joined+="${joined:+$separator}$flag"
done
printf '%s' "$joined"
