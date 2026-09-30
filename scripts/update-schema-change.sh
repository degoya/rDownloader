#!/usr/bin/env bash
#
# Whether a release changes the database schema (RD-180-02, RD-180-03): prints `true` or `false`
# for the update manifest's `schema_change`, which decides whether the backup before a
# self-update must include the encrypted full backup.
#
# Usage:
#   scripts/update-schema-change.sh <tag> [<repository>]
#
# The comparison is `crates/rd-db/migrations/` between <tag> and the release before it on its
# channel: for a plain vX.Y.Z the highest plain tag below it, for a vX.Y.Z-beta.N the highest beta
# below it (whatever version an installation on that channel runs, it is at least that one, so a
# change since then covers it). Anything this cannot tell — no earlier release, a tag that is not
# in the repository, git failing — prints `true`, the side that asks for the backup. The release
# workflow needs the tags fetched (`fetch-depth: 0`).
set -euo pipefail

[[ $# -ge 1 && $# -le 2 ]] || {
    echo "usage: scripts/update-schema-change.sh <tag> [<repository>]" >&2
    exit 2
}
TAG="$1"
REPO="${2:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
MIGRATIONS="crates/rd-db/migrations"

changed() {
    echo true
    [[ -n "${1:-}" ]] && echo "schema change: $1" >&2
    exit 0
}

git -C "$REPO" rev-parse --verify --quiet "refs/tags/$TAG" > /dev/null \
    || changed "$TAG is not a tag here"

# The release before $TAG on its channel, by SemVer precedence of the tag names.
previous="$(git -C "$REPO" tag --list 'v*' | python3 -c '
import re, sys
pattern = re.compile(r"^v(\d+)\.(\d+)\.(\d+)(?:-beta\.(\d+))?$")
def key(tag):
    major, minor, patch, beta = pattern.match(tag).groups()
    # A release outranks its betas.
    return (int(major), int(minor), int(patch), 1 if beta is None else 0, int(beta or 0))
tag = sys.argv[1]
if not pattern.match(tag):
    sys.exit(0)
beta = "-beta." in tag
earlier = [
    line.strip() for line in sys.stdin
    if pattern.match(line.strip())
    and ("-beta." in line) == beta
    and key(line.strip()) < key(tag)
]
if earlier:
    print(max(earlier, key=key))
' "$TAG")" || changed "the tags could not be read"
[[ -n "$previous" ]] || changed "no release before $TAG on its channel"

if git -C "$REPO" diff --quiet "$previous" "$TAG" -- "$MIGRATIONS"; then
    echo "no schema change since $previous" >&2
    echo false
else
    status=$?
    [[ "$status" -eq 1 ]] || changed "git diff failed"
    changed "$MIGRATIONS differs from $previous"
fi
