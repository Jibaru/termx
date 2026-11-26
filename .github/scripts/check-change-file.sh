#!/usr/bin/env bash
# Checks that a pull request which changes what users see adds a change file.
#
# The rule is derived from the Conventional Commit type of the pull request
# title: `feat`, `fix`, `perf` and breaking changes (`!`) need a change file
# in `.changeset/`. Every other type does not. The `no-changelog` label skips
# the check.
#
# Environment:
#   PR_TITLE        Title of the pull request.
#   SKIP_LABEL_SET  "true" when the pull request has the `no-changelog` label.
#   BASE_SHA        Commit the pull request is based on.
#   HEAD_SHA        Last commit of the pull request.
set -euo pipefail

if [[ "${SKIP_LABEL_SET:-false}" == "true" ]]; then
  echo "The 'no-changelog' label is set: no change file required."
  exit 0
fi

added="$(git diff --name-only --diff-filter=A "$BASE_SHA...$HEAD_SHA" -- '.changeset/*.md')"
if [[ -n "$added" ]]; then
  echo "Change files added by this pull request:"
  echo "$added"
  exit 0
fi

pattern='^([a-z]+)(\([^)]+\))?(!)?: '
if [[ ! "$PR_TITLE" =~ $pattern ]]; then
  echo "::error::The pull request title is not a Conventional Commit, so the change file rule cannot be evaluated: $PR_TITLE"
  exit 1
fi
type="${BASH_REMATCH[1]}"
breaking="${BASH_REMATCH[3]}"

if [[ -z "$breaking" && ! "$type" =~ ^(feat|fix|perf)$ ]]; then
  echo "Pull requests of type '$type' do not need a change file."
  exit 0
fi

echo "::error::A '$type${breaking}' pull request changes what users see and needs a change file."
cat <<'MESSAGE'

Add one with:

    knope document-change

and commit the file it creates in .changeset/.

If this pull request really needs no changelog entry, either give it a title
with a type that fits (docs, test, refactor, ci, build, chore, style) or ask a
maintainer to add the 'no-changelog' label.

See the "Changelog and commits" section of CONTRIBUTING.md.
MESSAGE
exit 1
