# shellcheck shell=bash
#
# The record of the public CI's greens, per runner image and tree, split from verified.sh
# (RD-1280-08), which sources it: callers source verified.sh and find these functions there.
# Expects verified.sh's rd_non_doc_paths, rd_is_version_file, rd_version_lines_only and
# rd_workspace_version.

# --- GitHub greens, per platform (RD-160-06) ---------------------------------------------------
#
# The public CI costs an hour per platform; its runners cost nothing, the repository being public
# (RD-1120-07), but the hour is the wave's critical path. A wave's integration branch goes
# through it on Linux and Windows; the release candidate made from it differs only in the version
# lines of the bump and the release's documentation, and ran all three platforms again anyway —
# 1.5.0 was released by hand without --push for that reason, with macOS dispatched alone. The
# record says which runner image was green for which tree, and a run only dispatches the images
# no record covers. A red run records nothing, so red still holds the merge.
#
# One file for the whole repository, in the common git directory: every worktree sees it, it is
# never tracked, and deleting target/ does not throw away greens that cost an hour each.

# The record file of the repository checkout $1 belongs to.
rd_ci_record_file() {
    printf '%s/rd-verified-ci\n' "$(git -C "$1" rev-parse --path-format=absolute --git-common-dir)"
}

# Whether trees $2 and $3 of checkout $1 hold the same content up to documentation and a version
# bump: every differing path is documentation (rd_non_doc_paths) or a version file, and in the
# version files every changed line carried the workspace version of $2 and carries the one of $3.
# The version files as a whole are compared, so a dependency moved in Cargo.lock does not pass.
rd_tree_same_but_versions() {
    local root="$1" from="$2" to="$3" changes path version_files=0 old new
    changes="$(git -C "$root" diff --name-only "$from" "$to" 2> /dev/null)" || return 1
    while read -r path; do
        [[ -n "$path" ]] || continue
        rd_is_version_file "$path" || return 1
        version_files=1
    done < <(rd_non_doc_paths <<< "$changes")
    [[ "$version_files" -eq 1 ]] || return 0
    old="$(git -C "$root" show "$from:Cargo.toml" 2> /dev/null | rd_workspace_version)"
    new="$(git -C "$root" show "$to:Cargo.toml" 2> /dev/null | rd_workspace_version)"
    [[ -n "$old" && -n "$new" && "$old" != "$new" ]] || return 1
    git -C "$root" diff -U0 "$from" "$to" -- "${RD_VERSION_FILES[@]}" | rd_version_lines_only "$old" "$new"
}

# Records that the public CI was green on runner image(s) $3... for tree $2 of checkout $1 — or
# for a release workflow, under its file name (`e2e.yml`, RD-1120-07).
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
