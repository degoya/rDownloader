#!/usr/bin/env bash
# shellcheck shell=bash
#
# The record of which revision a checkout last verified green, and the gate that stops a
# deferred state reaching a merge or a release.
#
# `scripts/check.sh --defer` exists so a translated string or a colour does not cost a forty
# minute run. The deferral is only sound if it expires: the next full run picks the postponed
# commits up by itself, because the change set is measured against the OLDER of the branch point
# and this marker, and `--defer` deliberately does not move the marker.
#
# Where the marker lives, and why not simply `$target_dir/.rd-verified`: target/ is SHARED
# between the main checkout and every feature worktree — that sharing is the whole reason the
# .rd-checkout stamp of check.sh exists — so a single file there would be overwritten by
# whichever checkout ran last, and one worktree's green would erase another's. One file per
# checkout root, in a directory beside the stamp, keeps the record per checkout as intended and
# still out of the source tree, where git would have to ignore it.

# The target directory of the checkout rooted at $1, by the same derivation check.sh,
# build-plugins.sh and the packaging scripts use.
rd_target_dir() {
    printf '%s\n' "${CARGO_TARGET_DIR:-$1/target}"
}

# The marker file for the checkout rooted at $1.
rd_verified_marker() {
    printf '%s/.rd-verified/%s\n' "$(rd_target_dir "$1")" "$(printf '%s' "$1" | tr '/' '%')"
}

# The revision that checkout $1 last verified, or nothing.
rd_verified_revision() {
    cat "$(rd_verified_marker "$1")" 2> /dev/null || true
}

# Records $2 as verified for the checkout rooted at $1.
rd_record_verified() {
    local marker; marker="$(rd_verified_marker "$1")"
    mkdir -p "$(dirname "$marker")"
    printf '%s\n' "$2" > "$marker"
}

# How many commits of checkout $1 have never been through a green run. Prints a number; a
# checkout with no marker at all prints the number of commits since its base branch, which is
# the honest answer to "how much has never been verified here".
rd_unverified_count() {
    local root="$1" base="${2:-development}" verified
    verified="$(rd_verified_revision "$root")"
    if [[ -n "$verified" ]] && git -C "$root" rev-parse -q --verify "$verified^{commit}" > /dev/null 2>&1; then
        git -C "$root" rev-list --count "$verified..HEAD" 2> /dev/null || echo 0
    else
        git -C "$root" rev-list --count "$base..HEAD" 2> /dev/null || echo 0
    fi
}

# Refuses unless checkout $1 has verified exactly its current HEAD. $2 names the caller in the
# message. This is the point at which a deferral is used up: a postponed state is not
# "tests pass", and point 1 of the Release & Delivery list is not met until this passes.
rd_verified_gate() {
    local root="$1" label="$2" head verified
    head="$(git -C "$root" rev-parse HEAD)"
    verified="$(rd_verified_revision "$root")"
    if [[ "$verified" == "$head" ]]; then
        echo "==> verified: $label is green at ${head:0:12}"
        return 0
    fi
    echo "!! $label has $(rd_unverified_count "$root") commit(s) that no green run has seen." >&2
    if [[ -n "$verified" ]]; then
        echo "   last verified: ${verified:0:12}" >&2
    else
        echo "   last verified: never in this checkout" >&2
    fi
    echo "   HEAD:          ${head:0:12}" >&2
    echo "   run scripts/check.sh --full there first; a deferred state is not 'tests pass'." >&2
    return 1
}

# --- the full green (RD-120-58) -------------------------------------------------------------
#
# A branch green is scoped: it proves what the change touched, one level out. A tag and a
# Windows package for the owner need more — a `check.sh --full` green on exactly the content
# being shipped. That record is kept by TREE, not by commit, because the release pipeline tests
# the working tree after its version bump and tags a merge commit on main afterwards: three
# commits, one content. A half run (`--rust` or `--web`, as the pipeline runs them) records its
# half; the gate wants both halves on the same tree.

# The tree of the working state of checkout $1: HEAD plus every change, tracked or not, that
# git does not ignore. Built in a copy of the index, so nothing the user staged is touched.
rd_worktree_tree() {
    local root="$1" index status=0 tree
    index="$(mktemp)"
    cp "$(git -C "$root" rev-parse --path-format=absolute --git-path index)" "$index" 2> /dev/null || rm -f "$index"
    tree="$(GIT_INDEX_FILE="$index" git -C "$root" add -A > /dev/null 2>&1 \
        && GIT_INDEX_FILE="$index" git -C "$root" write-tree)" || status=$?
    rm -f "$index"
    [[ "$status" -eq 0 ]] && printf '%s\n' "$tree"
}

# The full-green marker file of checkout $1.
rd_full_marker() {
    printf '%s/.rd-verified-full/%s\n' "$(rd_target_dir "$1")" "$(printf '%s' "$1" | tr '/' '%')"
}

# Records that a --full run covered half $2 (`rust` or `web`) of tree $3 in checkout $1.
rd_record_full() {
    local marker; marker="$(rd_full_marker "$1")"
    mkdir -p "$(dirname "$marker")"
    { grep -v "^$2 " "$marker" 2> /dev/null || true; printf '%s %s\n' "$2" "$3"; } > "$marker.tmp"
    mv "$marker.tmp" "$marker"
}

# Whether tree $2 differs from tree $3 of checkout $1 in documentation only — the same rule
# check.sh applies ("a documentation-only change gets no build"), crates/rd-core/recovery-matrix.md
# and crates/rd-api/mcp-coverage.md excepted because a test reads them. A tree git no longer has cannot be compared and does not qualify.
rd_tree_docs_only() {
    local changes
    changes="$(git -C "$1" diff --name-only "$2" "$3" 2> /dev/null)" || return 1
    [[ -n "$changes" ]] || return 0
    ! grep -qvE '^docs/|\.md$' <<< "$changes" \
        && ! grep -qxE 'crates/rd-core/recovery-matrix\.md|crates/rd-api/mcp-coverage\.md' <<< "$changes"
}

# Refuses unless both halves of a --full run are recorded for the current working state of
# checkout $1, or for a tree it differs from in documentation only — so the verification note
# written after the full run, and the changelog of the release commit, do not demand another.
# $2 names what is being gated, for the message.
rd_full_gate() {
    local root="$1" label="$2" tree half recorded missing=() docs=0
    tree="$(rd_worktree_tree "$root")"
    for half in rust web; do
        recorded="$(sed -n "s/^$half //p" "$(rd_full_marker "$root")" 2> /dev/null || true)"
        if [[ -n "$tree" && "$recorded" == "$tree" ]]; then
            continue
        elif [[ -n "$tree" && -n "$recorded" ]] && rd_tree_docs_only "$root" "$recorded" "$tree"; then
            docs=1
        else
            missing+=("$half")
        fi
    done
    if [[ ${#missing[@]} -eq 0 ]]; then
        echo "==> full green: $label is covered by a check.sh --full run$([[ "$docs" -eq 1 ]] \
            && echo ', documentation changed since') (tree ${tree:0:12})"
        return 0
    fi
    echo "!! $label needs a scripts/check.sh --full green on exactly this content." >&2
    echo "   missing for tree ${tree:0:12}: ${missing[*]}" >&2
    echo "   A branch green is scoped and does not count; run scripts/check.sh --full here." >&2
    return 1
}
