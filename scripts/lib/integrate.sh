# shellcheck shell=bash
#
# The merge half of scripts/integrate.sh (RD-140-22), kept apart so it can be tested against a
# scratch repository without anything that compiles, and the runner of its foreground checks.
#
# Sourced, never run. The functions return non-zero and say why; the caller decides to stop.

# The generated files, RD_GENERATED_FILES (inert-paths.sh): nobody edits them by hand, so a merge
# conflict in one of them has no side to take — either side is taken and the generator writes the
# truth after the last merge.
# shellcheck source=inert-paths.sh
source "$(dirname "${BASH_SOURCE[0]}")/inert-paths.sh"

# The merge drivers .gitattributes names (RD-1100-13), registered in the repository's config, which
# every worktree shares, so the merges resolve the files whose conflicts have one right answer
# without a person: crates/rd-db/migrations.sha384 as the sorted union of both sides
# (`rd-pins`), the locale catalogues key by key (`rd-json`), CHANGELOG.md by section, new entries
# under [Unreleased] even across a release (`rd-changelog`, RD-1220-01), and the two job indexes
# with conflicts of table rows only taken from both sides (`rd-jobindex`), which archive-jobs.sh
# after the last merge reduces to one row per job. A clone without the config merges those files
# as text. The drivers are the ones of checkout $2, by absolute path — the checkout integrate.sh
# runs from — and every run registers them again.
rd_integrate_merge_drivers() {
    local tree="$1" drivers="$2/scripts/lib/merge-drivers"
    git -C "$tree" config merge.rd-pins.name "migration pins: the sorted union of both sides"
    git -C "$tree" config merge.rd-pins.driver "'$drivers/migration-pins.sh' %O %A %B %P"
    git -C "$tree" config merge.rd-json.name "JSON catalogues: a three-way merge key by key"
    git -C "$tree" config merge.rd-json.driver "python3 '$drivers/json-merge.py' %O %A %B %P"
    git -C "$tree" config merge.rd-changelog.name "CHANGELOG: by section, new entries under [Unreleased]"
    git -C "$tree" config merge.rd-changelog.driver "python3 '$drivers/changelog-merge.py' %O %A %B %P"
    git -C "$tree" config merge.rd-jobindex.name "job indexes: conflicts of table rows take both sides"
    git -C "$tree" config merge.rd-jobindex.driver "python3 '$drivers/job-index-merge.py' %O %A %B %P"
}

rd_is_generated() {
    local file="$1" generated
    for generated in "${RD_GENERATED_FILES[@]}"; do
        [[ "$file" == "$generated" ]] && return 0
    done
    return 1
}

# Merges branch $2 into the checkout at $1. Already contained: says so and succeeds. A conflict
# only in generated files is resolved to our side and committed; any other conflict leaves the
# merge in progress, names the files and fails.
rd_integrate_merge() {
    local tree="$1" branch="$2" file
    local -a conflicts=() by_hand=()
    if git -C "$tree" merge-base --is-ancestor "$branch" HEAD; then
        echo "    $branch: already merged"
        return 0
    fi
    if git -C "$tree" merge --no-ff --no-edit --quiet "$branch" > /dev/null 2>&1; then
        echo "    $branch: merged"
        return 0
    fi
    mapfile -t conflicts < <(git -C "$tree" diff --name-only --diff-filter=U)
    if [[ ${#conflicts[@]} -eq 0 ]]; then
        echo "!! $branch: the merge failed without a conflict — read 'git -C $tree status'" >&2
        return 1
    fi
    for file in "${conflicts[@]}"; do
        rd_is_generated "$file" || by_hand+=("$file")
    done
    if [[ ${#by_hand[@]} -gt 0 ]]; then
        echo "!! $branch: conflicts to resolve by hand in $tree:" >&2
        printf '     %s\n' "${by_hand[@]}" >&2
        [[ ${#by_hand[@]} -eq ${#conflicts[@]} ]] || {
            echo "   the generated ones among the conflicts take either side — the generators run" >&2
            echo "   after the last merge:" >&2
            for file in "${conflicts[@]}"; do
                rd_is_generated "$file" && echo "     git checkout --ours -- $file && git add $file" >&2
            done
        }
        echo "   Resolve, 'git commit --no-edit', then run the same integrate.sh command again:" >&2
        echo "   merged branches are skipped." >&2
        return 1
    fi
    git -C "$tree" checkout --ours -- "${conflicts[@]}"
    git -C "$tree" add -- "${conflicts[@]}"
    git -C "$tree" commit --quiet --no-edit
    echo "    $branch: merged; generated files in conflict took our side, the generators rewrite them:"
    printf '      %s\n' "${conflicts[@]}"
}

# What two branches can each get right and still get wrong together, because neither sees the
# other: the same migration number, the same plugin id. Prints each duplicate; fails on one.
rd_integrate_duplicates() {
    local tree="$1" migrations ids number line
    migrations="$(find "$tree/crates/rd-db/migrations" -maxdepth 1 -name '*.sql' -printf '%f\n' 2> /dev/null \
        | cut -d_ -f1 | sort | uniq -d || true)"
    ids="$(cat "$tree"/plugins/*/manifest.toml 2> /dev/null | grep '^id = ' | sort | uniq -d || true)"
    [[ -z "$migrations" ]] || while read -r number; do
        echo "duplicate migration number $number:" \
            "$(cd "$tree/crates/rd-db/migrations" && printf '%s ' "$number"_*.sql)"
    done <<< "$migrations"
    [[ -z "$ids" ]] || while read -r line; do
        echo "duplicate plugin $line:" \
            "$(cd "$tree" && grep -lxF "$line" plugins/*/manifest.toml | tr '\n' ' ')"
    done <<< "$ids"
    [[ -z "$migrations" && -z "$ids" ]]
}

# Runs check $3... in the foreground with RD_CHECK_LOGS=$1, into $1/$2.log, judged by its closing
# line, not its exit code alone. Red names every finding from the failure list, says that nothing
# was generated or built, and fails; green says so.
rd_integrate_check() {
    local logs="$1" name="$2" status=0
    shift 2
    RD_CHECK_LOGS="$logs" "$@" > "$logs/$name.log" 2>&1 || status=$?
    echo "REAL EXIT: $status" >> "$logs/$name.log"
    if [[ "$status" -eq 0 ]] && grep -qx '==> all requested checks passed' "$logs/$name.log"; then
        echo "    green"
        return 0
    fi
    echo "!! the $name is red (exit $status); nothing was generated or built." >&2
    if [[ -s "$logs/failures" ]]; then
        echo "   Every finding, from $logs/failures:" >&2
        sed 's/^/   /' "$logs/failures" >&2
    else
        echo "   No failure list was written; read $logs/$name.log." >&2
    fi
    echo "   Hand each back to the branch that owns it, then run the same integrate.sh again." >&2
    return 1
}
