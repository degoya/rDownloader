#!/usr/bin/env bash
#
# The release documents' mechanics, written instead of done by hand (RD-1270-15): the CHANGELOG
# cut with the list of plugin versions the release raises, the release notes' draft comment, and
# the next version's draft section. What the entries and the notes say stays judgement — this
# moves and lists, it writes no entry and no point.
#
#   scripts/release-docs.sh X.Y.Z [--date YYYY-MM-DD]
#
# The cut, run before the release chain (whose preflight wants a committed tree): CHANGELOG.md's
# `## [Unreleased]` becomes `## [X.Y.Z] - <date, default today>` under a new, empty [Unreleased];
# when a `plugins/<dir>/manifest.toml` version differs from the previous release's tag (the highest
# `vX.Y.Z` under the version, scripts/lib/release-tag.sh), the section's `### Changed` ends with
# one point "**Plugin versions.** The versions this release raises: `a` and `b` 0.1.13; …", grouped
# by version, wrapped at 100 columns. RELEASE-NOTES.md's `## X.Y.Z` section (a beta's falls back to
# its release's) loses its `draft` comment. Refused (exit 1, nothing written): a version cut
# already, no [Unreleased], or nothing under it. Named on stderr: a notes section missing or still
# without points, beta sections to consolidate. Review the diff, then commit it.
#
#   scripts/release-docs.sh --plugins X.Y.Z
#
# Prints that sentence alone ("The versions this release raises: …"), for an entry the list
# belongs to; nothing when no plugin version changed.
#
#   scripts/release-docs.sh --next-draft X.Y.Z
#
# After the release X.Y.Z: RELEASE-NOTES.md gets the next minor version's section on top
# (`## X.(Y+1).0` with the comment `<!-- draft: the notes of the version in the making -->`),
# unless it has one; a beta adds none. Idempotent, one line of output either way. Meant for the
# release chain's step before commit-guard, so the release commit carries it.
#
#   scripts/release-docs.sh --check X.Y.Z
#
# Whether the documents of the release X.Y.Z are ready (RD-1300-04): CHANGELOG.md has its
# `## [X.Y.Z] - YYYY-MM-DD` section, RELEASE-NOTES.md a finished one (`release-notes.sh --check
# --version`), docs/roadmap.md names X.Y.Z. Writes nothing; each finding names what fixes it, exit 1
# while any is left. One source for both callers: release-start.sh before anything detaches, and
# the chain's docs-gate (scripts/lib/release-steps-publish.sh), which on 1.29.0 found a roadmap
# without the version only after about 40 minutes.
#
# The rules in full: scripts/lib/release-docs.py. Tested by scripts/tests/release-docs.sh and
# scripts/tests/release-start-preconditions.sh.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/release-tag.sh
source "$ROOT/scripts/lib/release-tag.sh"
PY="$ROOT/scripts/lib/release-docs.py"

usage() {
    echo "usage: scripts/release-docs.sh X.Y.Z [--date YYYY-MM-DD]" >&2
    echo "       scripts/release-docs.sh --plugins X.Y.Z" >&2
    echo "       scripts/release-docs.sh --next-draft X.Y.Z" >&2
    echo "       scripts/release-docs.sh --check X.Y.Z" >&2
    exit 2
}

version_arg() {
    local version="${1#v}"
    rd_release_version "$version" || { echo "release-docs: '$1' is no release version (X.Y.Z or X.Y.Z-beta.N)" >&2; exit 2; }
    echo "$version"
}

previous_tag() { (cd "$ROOT" && rd_release_tag_at_most "$1" --below); }

# The documents --check holds the release to; findings on stderr, what holds on stdout.
check_ready() {
    local version="$1" failed=0
    cd "$ROOT"
    if grep -qE "^## \[${version//./\\.}\] - [0-9]{4}-[0-9]{2}-[0-9]{2}\$" CHANGELOG.md 2> /dev/null; then
        echo "CHANGELOG.md: $(awk -v v="## [$version]" '
            /^## \[/ { inside = (index($0, v) == 1) } inside && NF' CHANGELOG.md | wc -l) lines for $version"
    else
        echo "CHANGELOG.md has no '## [$version] - YYYY-MM-DD' section: run scripts/release-docs.sh $version, review the diff and commit it" >&2
        failed=1
    fi
    if ! scripts/release-notes.sh --check --version "$version"; then
        echo "RELEASE-NOTES.md: finish the section of $version (scripts/release-docs.sh $version drops its draft comment) and commit it" >&2
        failed=1
    fi
    if grep -qF "$version" docs/roadmap.md 2> /dev/null; then
        echo "docs/roadmap.md names $version"
    else
        echo "docs/roadmap.md never names $version: write it into the milestone's status line and commit it" >&2
        failed=1
    fi
    return "$failed"
}

case "${1:-}" in
    --plugins)
        [[ $# -eq 2 ]] || usage
        version="$(version_arg "$2")"
        exec python3 -B "$PY" plugins "$ROOT" "$(previous_tag "$version")"
        ;;
    --next-draft)
        [[ $# -eq 2 ]] || usage
        version="$(version_arg "$2")"
        exec python3 -B "$PY" next-draft "$ROOT" "$version"
        ;;
    --check)
        [[ $# -eq 2 ]] || usage
        version="$(version_arg "$2")"
        check_ready "$version"
        ;;
    -*|"") usage ;;
    *)
        version="$(version_arg "$1")"
        shift
        date="$(date +%F)"
        if [[ $# -gt 0 ]]; then
            [[ $# -eq 2 && "$1" == --date && "$2" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] || usage
            date="$2"
        fi
        exec python3 -B "$PY" cut "$ROOT" "$version" "$date" "$(previous_tag "$version")"
        ;;
esac
