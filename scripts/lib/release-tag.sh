# shellcheck shell=bash
#
# The last release a version comes after, as its tag: the highest `vX.Y.Z` tag not above the
# version, or with --below the highest one under it. Empty when there is none.
#
# Not "the tag reachable from HEAD": release tags sit on main's merge commits, which development
# never contains (on development after 1.5.2, `git describe` answers v1.4.2). An empty version
# takes the highest tag of all. compat-check.sh picks its default base with it (RD-170-08), the
# release pipeline's docs-gate the release its changelog is compared against.
#
#   source "$ROOT/scripts/lib/release-tag.sh"
#   rd_release_tag_at_most 1.6.1           # v1.6.1 when it exists, else v1.6.0
#   rd_release_tag_at_most 1.6.1 --below   # v1.6.0
rd_release_tag_at_most() {
    local version="${1:-}" below="${2:-}" tag highest
    while read -r tag; do
        [[ -n "$tag" ]] || continue
        if [[ -z "$version" ]]; then
            echo "$tag"; return 0
        fi
        [[ "$below" == --below && "${tag#v}" == "$version" ]] && continue
        highest="$(printf '%s\n%s\n' "${tag#v}" "$version" | sort -V | tail -1)"
        if [[ "$highest" == "$version" ]]; then
            echo "$tag"; return 0
        fi
    done < <(git tag --list 'v*' | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sort -rV || true)
}
