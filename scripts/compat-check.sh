#!/usr/bin/env bash
#
# The breaking-change gate of the two public contracts (RD-170-08): the REST API as
# web/openapi.json describes it, and the plugin contract rdownloader:plugin@X.Y.Z in
# crates/rd-plugin-api/wit/rdownloader.wit, each compared between a base release and the working
# tree. The rules are in scripts/lib/compat-check.py; the policy is docs/plugins.md#compatibility
# and the wiki's API reference.
#
# One line per break, with its location. A break passes when scripts/compat-breaks.toml lists
# it for a release after the base, with a reason — or, for the WIT, when the package version
# moved by a major (before 1.0: a minor) step. Anything else exits 1.
#
# The default base is the highest `vX.Y.Z` tag not above the workspace version. Not "the tag
# reachable from HEAD": release tags sit on main's merge commits, which development never
# contains (on development after 1.5.2, `git describe` answers v1.4.2).
#
# The release pipeline runs it as its `compat` step, CI in the supply-chain job.
#
# Usage:
#   scripts/compat-check.sh                 # against the last release
#   scripts/compat-check.sh --base v1.5.2   # against any ref
#
# Exit: 0 every break acknowledged or versioned, 1 an unacknowledged break, 2 no base or bad input.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
# shellcheck source=lib/release-tag.sh
source "$ROOT/scripts/lib/release-tag.sh"
# shellcheck source=lib/workspace-version.sh
source "$ROOT/scripts/lib/workspace-version.sh"

OPENAPI=web/openapi.json
WIT=crates/rd-plugin-api/wit/rdownloader.wit
ACKS=scripts/compat-breaks.toml

BASE=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --base) BASE="${2:?--base needs a ref}"; shift 2 ;;
        -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

# The [workspace.package] version of Cargo.toml as it stands at a ref, or in the working tree.
workspace_version() {
    local source
    if [[ -n "${1:-}" ]]; then
        source="$(git show "$1:Cargo.toml" 2>/dev/null)" || return 0
    else
        [[ -f Cargo.toml ]] || return 0
        source="$(cat Cargo.toml)"
    fi
    rd_workspace_version <<< "$source"
}

if [[ -z "$BASE" ]]; then
    current="$(workspace_version)"
    BASE="$(rd_release_tag_at_most "$current")"
    if [[ -z "$BASE" ]]; then
        echo "compat-check: no release tag vX.Y.Z at or below ${current:-the workspace version};" \
            "a shallow checkout needs the tags (git fetch --tags), or name one with --base" >&2
        exit 2
    fi
fi
git rev-parse -q --verify "$BASE^{commit}" > /dev/null \
    || { echo "compat-check: $BASE is no commit here" >&2; exit 2; }

base_version="$(workspace_version "$BASE")"
[[ -n "$base_version" ]] || base_version="$(sed -n 's/^v\{0,1\}\([0-9]*\.[0-9]*\.[0-9]*\)$/\1/p' <<< "$BASE")"
echo "compat-check: $BASE (${base_version:-version unknown}) -> working tree"

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
args=(--base-version "${base_version:-0.0.0}" --acks "$ACKS")
if git show "$BASE:$OPENAPI" > "$SCRATCH/old.json" 2>/dev/null; then
    args+=(--old-openapi "$SCRATCH/old.json" --new-openapi "$OPENAPI")
else
    echo "note: $BASE has no $OPENAPI; the REST API is not compared"
fi
if git show "$BASE:$WIT" > "$SCRATCH/old.wit" 2>/dev/null; then
    args+=(--old-wit "$SCRATCH/old.wit" --new-wit "$WIT")
else
    echo "note: $BASE has no $WIT; the plugin contract is not compared"
fi

PYTHONDONTWRITEBYTECODE=1 python3 "$ROOT/scripts/lib/compat-check.py" "${args[@]}"
