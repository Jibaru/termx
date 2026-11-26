#!/usr/bin/env bash
# Checks the change files of a pull request.
#
# 1. Every file in `.changeset/` must be a valid change file: front matter
#    with `default: major|minor|patch`, then a `# ` heading. Knope fails on
#    Markdown without front matter and silently drops entries with another key
#    or type, so anything else (a README, notes) is rejected.
# 2. Change files must not be deleted. They are removed by the release. The
#    exception is a `revert` pull request, which may delete the change file of
#    the change it reverts.
# 3. A pull request whose title has the Conventional Commit type `feat`, `fix`
#    or `perf`, or is marked as breaking (`!`), must add a change file. Every
#    other type does not need one. The `no-changelog` label skips this rule.
#
# Environment:
#   PR_TITLE        Title of the pull request.
#   SKIP_LABEL_SET  "true" when the pull request has the `no-changelog` label.
#   BASE_SHA        Commit the pull request is based on.
#   HEAD_SHA        Last commit of the pull request.
set -euo pipefail

docs='See the "Changelog and commits" section of CONTRIBUTING.md.'

# Succeeds when the file at "$HEAD_SHA:$1" is a valid change file.
is_change_file() {
  [[ "$1" == *.md ]] || return 1
  git show "$HEAD_SHA:$1" | awk '
    { sub(/\r$/, "") }
    NR == 1 { if ($0 != "---") exit 1; in_front = 1; next }
    in_front && $0 == "---" { in_front = 0; closed = 1; next }
    in_front && /^default: (major|minor|patch)[ \t]*$/ { bump = 1; next }
    closed && /^# [^ \t]/ { heading = 1 }
    END { exit !(closed && bump && heading) }
  '
}

# 1. Everything in .changeset/ is a valid change file.
invalid=()
while IFS= read -r path; do
  [[ -n "$path" ]] || continue
  is_change_file "$path" || invalid+=("$path")
done < <(git ls-tree -r --name-only "$HEAD_SHA" -- .changeset/)

if ((${#invalid[@]} > 0)); then
  echo "::error::Not a valid change file: ${invalid[*]}"
  cat <<MESSAGE

Every file in .changeset/ must be a change file that looks like this:

    ---
    default: minor
    ---

    # Summary of the change for users

    Optional details.

The value is major, minor or patch. Keep other files out of .changeset/.

$docs
MESSAGE
  exit 1
fi

pattern='^([a-z]+)(\([^)]+\))?(!)?: '
if [[ ! "$PR_TITLE" =~ $pattern ]]; then
  echo "::error::The pull request title is not a Conventional Commit, so the change file rules cannot be evaluated: $PR_TITLE"
  exit 1
fi
type="${BASH_REMATCH[1]}"
breaking="${BASH_REMATCH[3]}"

# 2. Change files are not deleted, except by a revert.
deleted="$(git diff --name-only --diff-filter=D "$BASE_SHA...$HEAD_SHA" -- .changeset/)"
if [[ -n "$deleted" && "$type" != "revert" ]]; then
  echo "::error::This pull request deletes change files: $(tr '\n' ' ' <<<"$deleted")"
  cat <<MESSAGE

Change files are deleted by the release, which moves them into CHANGELOG.md.
Restore them. Only a pull request of type 'revert' may delete the change file
of the unreleased change it reverts.

$docs
MESSAGE
  exit 1
fi

# 3. A user-visible change adds a change file.
if [[ "${SKIP_LABEL_SET:-false}" == "true" ]]; then
  echo "The 'no-changelog' label is set: no change file required."
  exit 0
fi

added="$(git diff --name-only --diff-filter=A "$BASE_SHA...$HEAD_SHA" -- .changeset/)"
if [[ -n "$added" ]]; then
  echo "Change files added by this pull request:"
  echo "$added"
  exit 0
fi

if [[ -z "$breaking" && ! "$type" =~ ^(feat|fix|perf)$ ]]; then
  echo "Pull requests of type '$type' do not need a change file."
  exit 0
fi

echo "::error::A '$type${breaking}' pull request changes what users see and needs a change file."
cat <<MESSAGE

Add one with:

    knope document-change

and commit the file it creates in .changeset/.

If this pull request really needs no changelog entry, either give it a title
with a type that fits (docs, test, refactor, ci, build, chore, style) or ask a
maintainer to add the 'no-changelog' label.

$docs
MESSAGE
exit 1
