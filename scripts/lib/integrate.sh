# shellcheck shell=bash
#
# The merge half of scripts/integrate.sh (RD-140-22), kept apart so it can be tested against a
# scratch repository without anything that compiles.
#
# Sourced, never run. The functions return non-zero and say why; the caller decides to stop.

# The generated files: nobody edits them by hand, so a merge conflict in one of them has no side
# to take — either side is taken and the generator writes the truth after the last merge.
RD_GENERATED_FILES=(
    web/openapi.json
    web/src/api/schema.d.ts
    crates/rd-api/mcp-coverage.md
    crates/rd-api/licenses/third-party.json
    web/components.d.ts
    web/auto-imports.d.ts
)

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
