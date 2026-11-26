#!/usr/bin/env bash
# Decides what the Release workflow has to do, from the checked-out commit.
#
# - `mode=finish`: the current version was prepared by an earlier run (it has
#   a section in CHANGELOG.md) but its tag `v<version>` does not exist, because
#   that run failed after pushing the release commit. The release of that
#   version must be finished before a new one is prepared.
# - `mode=prepare`: a new release can be prepared. Fails when there is no
#   change file in `.changeset/`, so that an empty release is never pushed,
#   even when a version is given by hand.
#
# Needs the full history and the tags. Writes `mode`, `version` (the current
# version) and `sha` (the release commit to tag, for `mode=finish`) to
# $GITHUB_OUTPUT, or to standard output when that is not set.
#
# Environment:
#   VERSION_INPUT  Optional version requested for a new release.
set -euo pipefail

output="${GITHUB_OUTPUT:-/dev/stdout}"

version="$(awk '
  /^\[/ { in_section = ($0 == "[workspace.package]") }
  in_section && /^version[ \t]*=/ { gsub(/[^0-9A-Za-z.+-]/, "", $3); print $3; exit }
' Cargo.toml)"
if [[ -z "$version" ]]; then
  echo "::error::Could not read the version from [workspace.package] in Cargo.toml."
  exit 1
fi
echo "version=$version" >>"$output"

prepared=false
if [[ -f CHANGELOG.md ]] && grep -qE "^## ${version//./\\.}( |\$)" CHANGELOG.md; then
  prepared=true
fi

if [[ "$prepared" == "true" ]] && ! git rev-parse -q --verify "refs/tags/v$version" >/dev/null; then
  # The release commit is the last one that set this version in Cargo.toml.
  sha="$(git log -1 --format=%H -G"^version = \"${version//./\\.}\"\$" -- Cargo.toml)"
  if [[ -z "$sha" ]]; then
    echo "::error::Version $version is in CHANGELOG.md and has no tag, but the commit that set it could not be found."
    exit 1
  fi
  echo "::notice::Version $version was prepared in $sha but never tagged. This run finishes that release and does not prepare a new one."
  if [[ -n "${VERSION_INPUT:-}" ]]; then
    echo "::warning::The requested version $VERSION_INPUT is ignored because version $version has to be finished first."
  fi
  {
    echo "mode=finish"
    echo "sha=$sha"
  } >>"$output"
  exit 0
fi

shopt -s nullglob
change_files=(.changeset/*.md)
if ((${#change_files[@]} == 0)); then
  echo "::error::There are no change files in .changeset/, so there is nothing to release."
  exit 1
fi

echo "Change files to release: ${change_files[*]}"
echo "mode=prepare" >>"$output"
