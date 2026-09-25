#!/usr/bin/env bash
#
# Tags the current commit as a release, after checking it is one.
#
# Deliberately separate from set-version.sh: the version is bumped *before* the release commit,
# and a tag has to point *at* it. Just as deliberately, this never pushes — a tag is the one
# thing here that other people see, so publishing it stays a decision rather than a side effect.
#
# A tag needs a `scripts/check.sh --full` green on the content it points at, documentation
# changes excepted (RD-120-58). Branch runs are scoped; the full run is the one that covers
# everything, and the release pipeline runs it on the tree it then commits, merges and tags.
# There is no override: the answer to a refusal is the full run.
#
# Usage:
#   scripts/tag-release.sh            # tag the version Cargo.toml reports
#   scripts/tag-release.sh 0.9.2      # tag that version, refusing if Cargo.toml disagrees
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/verified.sh
source "$ROOT/scripts/lib/verified.sh"
cd "$ROOT"

version="$(scripts/set-version.sh)"
if [[ $# -gt 0 && "$1" != "$version" ]]; then
    echo "asked to tag $1 but Cargo.toml reports $version" >&2
    echo "run scripts/set-version.sh $1 first, and commit it" >&2
    exit 1
fi
tag="v$version"

if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "the working tree has uncommitted changes; a tag must point at a finished commit" >&2
    git status --short >&2
    exit 1
fi

if git rev-parse -q --verify "refs/tags/$tag" > /dev/null; then
    echo "$tag already exists (pointing at $(git rev-parse --short "$tag"))" >&2
    exit 1
fi

rd_full_gate "$ROOT" "$tag" || exit 1

# A release without a changelog entry is almost always a version that was bumped and forgotten.
if ! grep -q "^## \[$version\]" CHANGELOG.md; then
    echo "CHANGELOG.md has no '## [$version]' section" >&2
    exit 1
fi

# The changelog section becomes the tag message, so `git show <tag>` says what shipped.
message="$(awk -v version="## [$version]" '
    $0 ~ "^## \\[" { inside = (index($0, version) == 1) }
    inside { print }
' CHANGELOG.md)"

git tag -a "$tag" -m "$message"
echo "==> tagged $tag at $(git rev-parse --short HEAD)"
echo "    not pushed; publish with: git push origin $tag"
