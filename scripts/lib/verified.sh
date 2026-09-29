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

# The target directory of the checkout rooted at $1 — rd_target_dir — is derived in lanes.sh, by
# the same rule check.sh, build-plugins.sh and the packaging scripts use.
# shellcheck source=lanes.sh
source "$(dirname "${BASH_SOURCE[0]}")/lanes.sh"

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

# The full-green marker file of checkout $1. It also keeps the Windows lint's green, as the half
# `windows` (check.sh --windows, RD-160-06); the gates read only `rust` and `web`.
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

# The paths on stdin that are not documentation, by the rule check.sh applies ("a
# documentation-only change gets no build"): docs/ and *.md are documentation,
# crates/rd-core/recovery-matrix.md and crates/rd-api/mcp-coverage.md excepted because a test
# reads them.
rd_non_doc_paths() {
    local paths
    paths="$(cat)"
    { grep -vE '^docs/|\.md$' <<< "$paths" || true
      grep -xE 'crates/rd-core/recovery-matrix\.md|crates/rd-api/mcp-coverage\.md' <<< "$paths" || true
    } | sed '/^$/d'
}

# Whether tree $2 differs from tree $3 of checkout $1 in documentation only (rd_non_doc_paths).
# A tree git no longer has cannot be compared and does not qualify.
rd_tree_docs_only() {
    local changes
    changes="$(git -C "$1" diff --name-only "$2" "$3" 2> /dev/null)" || return 1
    [[ -z "$(rd_non_doc_paths <<< "$changes")" ]]
}

# The tree a --full green of half $2 was recorded for that covers tree $3 of checkout $1: that
# very tree, or one it differs from in documentation only; prints nothing when there is none.
# Every checkout that shares the target directory counts, not only $1 (RD-160-06): a tree is
# content, and the integration worktree's green is development's after the fast-forward merge —
# the release chain of 2026-09-28 ran --full again for want of this and of the documentation rule.
rd_full_covering() {
    local root="$1" half="$2" tree="$3" directory recorded candidate
    directory="$(dirname "$(rd_full_marker "$root")")"
    [[ -n "$tree" && -d "$directory" ]] || return 0
    recorded="$(cat "$directory"/* 2> /dev/null | sed -n "s/^$half //p" | sort -u || true)"
    if grep -qxF "$tree" <<< "$recorded"; then
        printf '%s\n' "$tree"
        return 0
    fi
    while read -r candidate; do
        [[ -n "$candidate" ]] || continue
        if rd_tree_docs_only "$root" "$candidate" "$tree"; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done <<< "$recorded"
}

# Refuses unless both halves of a --full run are recorded for the current working state of
# checkout $1, or for a tree it differs from in documentation only (rd_full_covering) — so the
# verification note written after the full run, and the changelog of the release commit, do not
# demand another. $2 names what is being gated, for the message.
rd_full_gate() {
    local root="$1" label="$2" tree half recorded missing=() docs=0
    tree="$(rd_worktree_tree "$root")"
    for half in rust web; do
        recorded="$(rd_full_covering "$root" "$half" "$tree")"
        if [[ -z "$recorded" ]]; then
            missing+=("$half")
        elif [[ "$recorded" != "$tree" ]]; then
            docs=1
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

# Whether a run of half or halves $2... in checkout $1 — `rust` and `web` of --full, `windows` of
# check.sh --windows — would only check content a green already covers (rd_full_covering,
# RD-160-06). If so it names the green, records the halves for the current tree — so the gates and
# the release chain's pre-bump rule find them — and returns 0; check.sh then ends there.
# Otherwise it prints nothing and returns 1.
rd_full_already_green() {
    local root="$1" tree half recorded
    local -a lines=()
    shift
    [[ $# -gt 0 ]] || return 1
    tree="$(rd_worktree_tree "$root")"
    [[ -n "$tree" ]] || return 1
    for half in "$@"; do
        recorded="$(rd_full_covering "$root" "$half" "$tree")"
        [[ -n "$recorded" ]] || return 1
        if [[ "$recorded" == "$tree" ]]; then
            lines+=("$half: tree ${recorded:0:12}, this very content")
        else
            lines+=("$half: tree ${recorded:0:12}, documentation changed since")
        fi
    done
    echo "==> a recorded green already covers this content ($*); it is not run again"
    printf '    %s\n' "${lines[@]}"
    for half in "$@"; do rd_record_full "$root" "$half" "$tree"; done
    echo "    recorded for tree ${tree:0:12} in $(rd_full_marker "$root")"
    echo "    (--again runs it anyway)"
}

# --- a release that does not test a tested tree twice (RD-140-06) -----------------------------
#
# The release chain bumps the version and then ran the whole Rust suite again, ~20 minutes, on a
# tree that differs from one a `check.sh --full` had just passed only in the version strings.
# The version files are what scripts/set-version.sh writes; a bump counts as version-only when
# every line it changed in them is a `version` line, so an edit that rode along in Cargo.toml or
# Cargo.lock still gets its full run.
RD_VERSION_FILES=(Cargo.toml Cargo.lock web/package.json extension/manifest.base.json web/openapi.json)

# The workspace version in the Cargo.toml text on stdin, as scripts/set-version.sh reads it.
rd_workspace_version() {
    sed -n '/^\[workspace\.package\]/,/^\[/p' | sed -n 's/^version = "\(.*\)"/\1/p' | head -1
}

# Whether the working tree of checkout $1 differs from $2 (default HEAD) only by a version bump:
# nothing untracked, no path but the version files, and in them no line but one that carried
# the old workspace version and now carries the new one. A dependency's `version` line in
# Cargo.lock looks the same and is exactly what must not pass, so the versions are compared, not
# just the key.
rd_version_bump_only() {
    local root="$1" base="${2:-HEAD}" changed old new path file allowed
    changed="$({ git -C "$root" diff --name-only "$base"; git -C "$root" ls-files --others --exclude-standard; } | sed '/^$/d' | sort -u)"
    [[ -n "$changed" ]] || return 1
    while read -r path; do
        allowed=0
        for file in "${RD_VERSION_FILES[@]}"; do [[ "$path" == "$file" ]] && allowed=1; done
        [[ "$allowed" -eq 1 ]] || return 1
    done <<< "$changed"
    old="$(git -C "$root" show "$base:Cargo.toml" 2> /dev/null | rd_workspace_version)"
    new="$(rd_workspace_version < "$root/Cargo.toml")"
    [[ -n "$old" && -n "$new" && "$old" != "$new" ]] || return 1
    git -C "$root" diff -U0 "$base" -- "${RD_VERSION_FILES[@]}" | rd_version_lines_only "$old" "$new"
}

# Whether every line the -U0 diff on stdin removed carried workspace version $1 and every line
# it added carries $2.
rd_version_lines_only() {
    local old="${1//./\\.}" new="${2//./\\.}" lines
    lines="$(grep -E '^[-+]' | grep -vE '^(\+\+\+|---) ' || true)"
    ! grep -vE "^-[[:space:]]*(\"version\": \"$old\",?|version = \"$old\")\$" <<< "$lines" \
        | grep -qvE "^\+[[:space:]]*(\"version\": \"$new\",?|version = \"$new\")\$"
}

# The tree a `--full` Rust green was recorded for in checkout $1 when that tree is HEAD's own and
# the working tree differs from HEAD only by a version bump; prints nothing otherwise. HEAD is
# the state before the bump, because the release chain commits the bump only later.
rd_prebump_full_green() {
    local root="$1" before recorded
    before="$(git -C "$root" rev-parse 'HEAD^{tree}')"
    recorded="$(sed -n 's/^rust //p' "$(rd_full_marker "$root")" 2> /dev/null || true)"
    [[ -n "$recorded" && "$recorded" == "$before" ]] || return 0
    rd_version_bump_only "$root" HEAD || return 0
    printf '%s\n' "$recorded"
}

# --- GitHub greens, per platform (RD-160-06) ---------------------------------------------------
#
# The public CI costs an hour and paid minutes per platform. A wave's integration branch goes
# through it on Linux and Windows; the release candidate made from it differs only in the version
# lines of the bump and the release's documentation, and ran all three platforms again anyway —
# 1.5.0 was released by hand without --push for that reason, with macOS dispatched alone. The
# record says which runner image was green for which tree, and a run only dispatches the images
# no record covers. A red run records nothing, so red still holds the merge.
#
# One file for the whole repository, in the common git directory: every worktree sees it, it is
# never tracked, and deleting target/ does not throw away greens that cost money.

# The record file of the repository checkout $1 belongs to.
rd_ci_record_file() {
    printf '%s/rd-verified-ci\n' "$(git -C "$1" rev-parse --path-format=absolute --git-common-dir)"
}

# Whether trees $2 and $3 of checkout $1 hold the same content up to documentation and a version
# bump: every differing path is documentation (rd_non_doc_paths) or a version file, and in the
# version files every changed line carried the workspace version of $2 and carries the one of $3.
# The version files as a whole are compared, so a dependency moved in Cargo.lock does not pass.
rd_tree_same_but_versions() {
    local root="$1" from="$2" to="$3" changes path file allowed version_files=0 old new
    changes="$(git -C "$root" diff --name-only "$from" "$to" 2> /dev/null)" || return 1
    while read -r path; do
        [[ -n "$path" ]] || continue
        allowed=0
        for file in "${RD_VERSION_FILES[@]}"; do [[ "$path" == "$file" ]] && allowed=1; done
        [[ "$allowed" -eq 1 ]] || return 1
        version_files=1
    done < <(rd_non_doc_paths <<< "$changes")
    [[ "$version_files" -eq 1 ]] || return 0
    old="$(git -C "$root" show "$from:Cargo.toml" 2> /dev/null | rd_workspace_version)"
    new="$(git -C "$root" show "$to:Cargo.toml" 2> /dev/null | rd_workspace_version)"
    [[ -n "$old" && -n "$new" && "$old" != "$new" ]] || return 1
    git -C "$root" diff -U0 "$from" "$to" -- "${RD_VERSION_FILES[@]}" | rd_version_lines_only "$old" "$new"
}

# Records that the public CI was green on runner image(s) $3... for tree $2 of checkout $1.
rd_record_ci() {
    local root="$1" tree="$2" file image
    shift 2
    file="$(rd_ci_record_file "$root")"
    for image in "$@"; do
        printf '%s %s %s\n' "$image" "$tree" "$(date -Is)" >> "$file"
    done
}

# The tree whose recorded green on runner image $3 covers tree $2 of checkout $1: that tree, or
# one it differs from only in documentation and version lines. Nothing when none does.
rd_ci_covering() {
    local root="$1" tree="$2" image="$3" recorded candidate
    recorded="$(awk -v image="$image" '$1 == image { print $2 }' "$(rd_ci_record_file "$root")" 2> /dev/null | sort -u || true)"
    if grep -qxF "$tree" <<< "$recorded"; then
        printf '%s\n' "$tree"
        return 0
    fi
    while read -r candidate; do
        [[ -n "$candidate" ]] || continue
        if rd_tree_same_but_versions "$root" "$candidate" "$tree"; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done <<< "$recorded"
}
