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
# What a changed path asks of a green: rd_inert_path, rd_non_inert_paths, rd_paths_read_by.
# shellcheck source=inert-paths.sh
source "$(dirname "${BASH_SOURCE[0]}")/inert-paths.sh"

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
# git does not ignore. Built in a copy of the index, so nothing the user staged is touched. The
# copy keeps the index's modification time (cp -p): git takes a file whose stat data matches its
# entry as unchanged unless the entry is as new as the index, so a copy dated now passed over a
# same-size rewrite in the second of the last index write and named a tree that was not the
# working state (RD-1270-06, check-reuse-crates.sh under load).
rd_worktree_tree() {
    local root="$1" index status=0 tree
    index="$(mktemp)"
    cp -p "$(git -C "$root" rev-parse --path-format=absolute --git-path index)" "$index" 2> /dev/null || rm -f "$index"
    tree="$(GIT_INDEX_FILE="$index" git -C "$root" add -A > /dev/null 2>&1 \
        && GIT_INDEX_FILE="$index" git -C "$root" write-tree)" || status=$?
    rm -f "$index"
    [[ "$status" -eq 0 ]] && printf '%s\n' "$tree"
}

# The full-green marker file of checkout $1. It also keeps the Windows lint's green, as the half
# `windows` (check.sh --windows, RD-160-06); the gates read only `rust` and `web`.
rd_full_marker() {
    printf '%s/.rd-verified-full/%s\n' "$(rd_target_dir "$1")" "${1//\//%}"
}

# Records that a --full run covered half $2 (`rust` or `web`) of tree $3 in checkout $1.
rd_record_full() {
    local marker; marker="$(rd_full_marker "$1")"
    mkdir -p "$(dirname "$marker")"
    { grep -v "^$2 " "$marker" 2> /dev/null || true; printf '%s %s\n' "$2" "$3"; } > "$marker.tmp"
    mv "$marker.tmp" "$marker"
}

# The paths on stdin that are not documentation, by the one rule every check applies
# (rd_inert_path, scripts/lib/inert-paths.sh).
rd_non_doc_paths() {
    rd_non_inert_paths
}

# Whether tree $2 differs from tree $3 of checkout $1 in documentation only (rd_non_doc_paths).
# A tree git no longer has cannot be compared and does not qualify.
rd_tree_docs_only() {
    local changes
    changes="$(git -C "$1" diff --no-renames --name-only "$2" "$3" 2> /dev/null)" || return 1
    [[ -z "$(rd_non_doc_paths <<< "$changes")" ]]
}

# Whether tree $3 differs from tree $4 of checkout $1 only in paths half $2 does not read
# (rd_paths_read_by, audit C1): the generated web files of integrate.sh's commit after the gate
# are nothing the Windows lint reads, so its green of the gate's tree still covers the merged one.
# One process, the diff: each path is weighed in bash (rd_path_read_by, RD-1270-06).
rd_tree_unread_by() {
    local changes path
    changes="$(git -C "$1" diff --no-renames --name-only "$3" "$4" 2> /dev/null)" || return 1
    rd_read_patterns_load
    while IFS= read -r path; do
        ! rd_path_read_by "$2" "$path" || return 1
    done <<< "$changes"
}

# --- the recorded greens rd_full_covering compares a tree with (RD-1270-06) -------------------
#
# One list per shell. Until 1.27 every call read every marker under <target>/.rd-verified-full/
# again, sorted the trees by hash and walked all of them — on 2026-10-12 161 preflight and 61 Rust greens,
# one `git diff` and six more processes each — and a check.sh run asks five times or more (each
# half of the reuse plan, the preflight, rd_full_already_green, the gate's lints, the release
# chain's three pre-bump questions).
#
#   * The list is read once per shell and read again only when a marker changed: every call reads
#     the markers in bash, without a process, and compares them with what the list was built from;
#     only a change costs the one `ls -t` that orders them. A caller that asks from command
#     substitutions loads it first (rd_full_candidates_load), so they all share it.
#   * Newest first, by the time its marker was last written (the checkout's latest green of any
#     half): a tree never comes behind one whose marker was written before its own. Markers of
#     the same time come by name, bytewise (ls -t's tie-break under LC_ALL=C), so one set of
#     markers gives one order on every machine and locale.
#   * Capped: rd_full_covering diffs at most RD_FULL_CANDIDATES_MAX trees of a half (default 32).
#     A marker keeps one tree per half and checkout, and a green that covers a tree is in practice
#     one of the last few — the integration branch's, the checkout's own, the tree before the
#     bump. 32 is more than a wave's worktrees, its integration branch and the main checkout
#     together: a green is passed over only after 32 other trees of its half were recorded in
#     markers written since. The very tree is still found among all of them, without a diff.

RD_FULL_CANDIDATES_MAX="${RD_FULL_CANDIDATES_MAX:-32}"

# The list: the checkout and target it was asked for, the marker directory and the markers it was
# built from, and per half the recorded trees, newest first, one per line, each once.
RD_FULL_CANDIDATES_FOR=""
RD_FULL_CANDIDATES_DIRECTORY=""
RD_FULL_CANDIDATES_SIGNATURE=""
declare -gA RD_FULL_CANDIDATES=()

# Loads the list for the target directory of checkout $1, unless it is loaded and no marker changed
# since; also builds the patterns rd_path_read_by reads. Without a process when nothing changed.
rd_full_candidates_load() {
    local marker directory file name content signature="" names line half tree
    local -A marker_texts=() candidate_seen=() marker_placed=()
    local -a markers_globbed=() markers_ordered=()
    rd_read_patterns_load
    # Where the markers lie costs git processes (rd_target_dir); asked once per checkout and target.
    if [[ "$1|${CARGO_TARGET_DIR:-}" == "$RD_FULL_CANDIDATES_FOR" ]]; then
        directory="$RD_FULL_CANDIDATES_DIRECTORY"
    else
        marker="$(rd_full_marker "$1")"
        directory="${marker%/*}"
    fi
    if [[ -d "$directory" ]]; then
        for file in "$directory"/*; do
            [[ -f "$file" ]] || continue
            name="${file##*/}" content=""
            { IFS= read -r -d '' content < "$file"; } 2> /dev/null || true
            marker_texts["$name"]="$content"
            markers_globbed+=("$name")
            signature+="$name"$'\n'"$content"$'\x1e'
        done
    fi
    RD_FULL_CANDIDATES_FOR="$1|${CARGO_TARGET_DIR:-}"
    [[ "$directory" != "$RD_FULL_CANDIDATES_DIRECTORY" || "$signature" != "$RD_FULL_CANDIDATES_SIGNATURE" ]] \
        || return 0
    RD_FULL_CANDIDATES=()
    RD_FULL_CANDIDATES_DIRECTORY="$directory"
    RD_FULL_CANDIDATES_SIGNATURE="$signature"
    [[ ${#markers_globbed[@]} -gt 0 ]] || return 0
    # shellcheck disable=SC2012  # the order by modification time is what is asked; the names are ours
    names="$(LC_ALL=C ls -t -- "$directory" 2> /dev/null)" || true
    while IFS= read -r name; do
        [[ -n "$name" && -n "${marker_texts[$name]+x}" && -z "${marker_placed[$name]+x}" ]] || continue
        markers_ordered+=("$name")
        marker_placed["$name"]=1
    done <<< "$names"
    # A marker written between the glob and ls goes last rather than nowhere.
    for name in "${markers_globbed[@]}"; do [[ -n "${marker_placed[$name]+x}" ]] || markers_ordered+=("$name"); done
    for name in "${markers_ordered[@]}"; do
        while IFS= read -r line; do
            [[ "$line" == *' '* ]] || continue
            half="${line%% *}" tree="${line#* }"
            [[ -z "${candidate_seen["$half $tree"]+x}" ]] || continue
            candidate_seen["$half $tree"]=1
            RD_FULL_CANDIDATES["$half"]+="$tree"$'\n'
        done <<< "${marker_texts[$name]}"
    done
    return 0
}

# The tree a green of half $2 was recorded for that covers tree $3 of checkout $1: that very tree,
# or one it differs from only in what that half does not read (rd_tree_unread_by) — documentation
# for every half; prints nothing when there is none. Every checkout that shares the target
# directory counts, not only $1 (RD-160-06): a tree is content, and the integration worktree's
# green is development's after the fast-forward merge — the release chain of 2026-09-28 ran
# --full again for want of this and of the documentation rule. The candidates come newest first
# from the shell's one list, and at most RD_FULL_CANDIDATES_MAX of them are diffed, one git
# process each; the very tree is looked up among all (rd_full_candidates_load, RD-1270-06).
rd_full_covering() {
    local root="$1" half="$2" tree="$3" candidate compared=0
    [[ -n "$tree" ]] || return 0
    rd_full_candidates_load "$root"
    if [[ $'\n'"${RD_FULL_CANDIDATES[$half]-}" == *$'\n'"$tree"$'\n'* ]]; then
        printf '%s\n' "$tree"
        return 0
    fi
    while IFS= read -r candidate; do
        [[ -n "$candidate" ]] || continue
        [[ "$compared" -lt "$RD_FULL_CANDIDATES_MAX" ]] || return 0
        compared=$((compared + 1))
        if rd_tree_unread_by "$root" "$half" "$candidate" "$tree"; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done <<< "${RD_FULL_CANDIDATES[$half]-}"
}

# Every tree a green of half $2 was recorded for by any checkout sharing the target directory of
# checkout $1, one per line, each once, newest first (rd_full_candidates_load).
rd_full_recorded() {
    rd_full_candidates_load "$1"
    printf '%s' "${RD_FULL_CANDIDATES[$2]-}"
}

# How recorded tree $3 of half $2 relates to tree $4 in checkout $1, for a message.
rd_covering_note() {
    if [[ "$3" == "$4" ]]; then
        echo "this very content"
    elif rd_tree_docs_only "$1" "$3" "$4"; then
        echo "documentation changed since"
    else
        echo "only what $2 does not read changed since"
    fi
}

# Refuses unless both halves of a --full run are recorded for the current working state of
# checkout $1, or for a tree it differs from only in what that half does not read, documentation
# above all (rd_full_covering) — so the
# verification note written after the full run, and the changelog of the release commit, do not
# demand another. $2 names what is being gated, for the message.
rd_full_gate() {
    local root="$1" label="$2" tree half recorded missing=() docs=0
    tree="$(rd_worktree_tree "$root")"
    rd_full_candidates_load "$root"
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
            && echo ', only what it does not read changed since') (tree ${tree:0:12})"
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
    rd_full_candidates_load "$root"
    for half in "$@"; do
        recorded="$(rd_full_covering "$root" "$half" "$tree")"
        [[ -n "$recorded" ]] || return 1
        lines+=("$half: tree ${recorded:0:12}, $(rd_covering_note "$root" "$half" "$recorded" "$tree")")
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
# The version files are what scripts/set-version.sh writes, RD_VERSION_FILES — paths and globs,
# the one list both read (lib/workspace-version.sh, RD-1270-09); a bump counts as version-only
# when every line it changed in them is a `version` line, so an edit that rode along in
# Cargo.toml or Cargo.lock still gets its full run.

# rd_workspace_version: the workspace version in the Cargo.toml text on stdin; RD_VERSION_FILES.
# shellcheck source=workspace-version.sh
source "$(dirname "${BASH_SOURCE[0]}")/workspace-version.sh"

# Whether path $1 is one of RD_VERSION_FILES, a glob entry matched as a pattern; a Windows `\`
# separator counts as `/`.
rd_is_version_file() {
    local file path="${1//\\//}"
    for file in "${RD_VERSION_FILES[@]}"; do
        # shellcheck disable=SC2053  # the entry is a glob on purpose
        [[ "$path" == $file ]] && return 0
    done
    return 1
}

# Whether the working tree of checkout $1 differs from $2 (default HEAD) only by a version bump:
# nothing untracked, no path but the version files, and in them no line but one that carried
# the old workspace version and now carries the new one. A dependency's `version` line in
# Cargo.lock looks the same and is exactly what must not pass, so the versions are compared, not
# just the key.
rd_version_bump_only() {
    local root="$1" base="${2:-HEAD}" changed old new path
    changed="$({ git -C "$root" diff --no-renames --name-only "$base"; git -C "$root" ls-files --others --exclude-standard; } | sed '/^$/d' | sort -u)"
    [[ -n "$changed" ]] || return 1
    while read -r path; do
        rd_is_version_file "$path" || return 1
    done <<< "$changed"
    old="$(git -C "$root" show "$base:Cargo.toml" 2> /dev/null | rd_workspace_version)"
    new="$(rd_workspace_version < "$root/Cargo.toml")"
    [[ -n "$old" && -n "$new" && "$old" != "$new" ]] || return 1
    git -C "$root" diff -U0 "$base" -- "${RD_VERSION_FILES[@]}" | rd_version_lines_only "$old" "$new"
}

# Whether every line the -U0 diff on stdin removed carried workspace version $1 and every line
# it added carries $2. A JSON line may carry the version without its pre-release suffix, as
# extension/manifest.base.json does (a browser takes integers only; scripts/set-version.sh), so
# 1.7.0 -> 1.8.0-beta.1 writes `"version": "1.8.0"` there. The TOML form stays exact: a
# dependency in Cargo.lock that happens to sit at the base version must not pass. So do the
# SDK workflows' `RDOWNLOADER_VERSION:` pin, which names the release tag, suffix included, and
# the SDK templates' `min_app_version` (RD-1270-09).
rd_version_lines_only() {
    local old new old_base new_base lines
    old="$(rd_version_pattern "$1")" new="$(rd_version_pattern "$2")"
    old_base="$(rd_version_pattern "${1%%[-+]*}")" new_base="$(rd_version_pattern "${2%%[-+]*}")"
    lines="$(grep -E '^[-+]' | grep -vE '^(\+\+\+|---) ' || true)"
    ! grep -vE "^-[[:space:]]*(\"version\": \"($old|$old_base)\",?|(min_app_)?version = \"$old\"|RDOWNLOADER_VERSION: $old)\$" <<< "$lines" \
        | grep -qvE "^\+[[:space:]]*(\"version\": \"($new|$new_base)\",?|(min_app_)?version = \"$new\"|RDOWNLOADER_VERSION: $new)\$"
}

# Version $1 as an extended regular expression matching exactly it.
rd_version_pattern() {
    local pattern="${1//./\\.}"
    printf '%s\n' "${pattern//+/\\+}"
}

# The tree a green of half $2 (default `rust`) covers in checkout $1 when it covers HEAD's own
# tree (rd_full_covering) and the working tree differs from HEAD only by a version bump; prints
# nothing otherwise. HEAD is the state before the bump, because the release chain commits the
# bump only later. The chain asks it for its test (`rust`), clippy (`clippy`) and web (`web`)
# steps (RD-1120-06, audit A5).
rd_prebump_green() {
    local root="$1" half="${2:-rust}" before recorded
    before="$(git -C "$root" rev-parse 'HEAD^{tree}')"
    recorded="$(rd_full_covering "$root" "$half" "$before")"
    [[ -n "$recorded" ]] || return 0
    rd_version_bump_only "$root" HEAD || return 0
    printf '%s\n' "$recorded"
}

rd_prebump_full_green() {
    rd_prebump_green "$1" rust
}

# The GitHub greens, per platform (RD-160-06): rd_ci_record_file, rd_tree_same_but_versions,
# rd_record_ci, rd_ci_covering — a file of their own since RD-1280-08, sourced here so every
# caller of this file keeps them.
# shellcheck source=verified-ci.sh
source "$(dirname "${BASH_SOURCE[0]}")/verified-ci.sh"
