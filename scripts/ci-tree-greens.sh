#!/usr/bin/env bash
#
# Which runner images ci.yml still has to check for a tree (RD-191-09 T02). A release pushes
# `main` with a tree the release chain already had checked as `ci/<version>` — one fresh commit
# per release in the public repository, so the commit differs and the tree does not — and ci.yml
# ran everything over it again, macOS included: 61 minutes for nothing. The `gate` job of ci.yml
# asks this script on a push to `main` and runs the platform jobs only for the images it names;
# with none left only `components` runs, whose cache release.yml reads. The jobs that run once
# per run rather than per image (`docker`, `s3-live`, `clamav-live`, `supply-chain`, `scripts`)
# are named after `--` and decided the same way, so a push that misses macOS alone does not run
# them all again (RA-TOOL-05).
#
# An image counts as checked when a successful run of ci.yml — any branch, any event — has a head
# commit with this very tree and a successful `rust (<image>)` job. Nothing else counts: a tree
# that differs in one byte, a red run, a run that did not take that image (a dispatch for macOS
# alone covers macOS only). Every run is looked at in full; the jobs a dispatch left out are not in
# a successful run's list, so the union of runs is what decides, per image. A job after `--`
# counts when such a run has a successful job of that name, or of a leg `<job> (…)` of its matrix;
# in a successful run no leg failed.
#
# e2e.yml and recovery.yml ask the same through scripts/ci-gate.sh, as their own `gate` job
# (RD-1270-11): the release candidate dispatches both on `ci/<version>` before the tag, and the
# push to `main` after it carried the same tree through them again. CI_TREE_GREENS_WORKFLOW names
# the workflow whose runs count (ci.yml), CI_TREE_GREENS_LEG the matrix job whose legs
# `<leg> (<image>)` check an image (`rust`; e2e.yml's `smoke`). recovery.yml has no matrix: it
# names no image and its one job after `--`, by the name its check carries.
#
# Their greens come from a tree that is not quite the one pushed to `main`: the wave's integration
# branch, which the release candidate then differs from only in the version bump and the release's
# documentation (RD-1270-10). With CI_TREE_GREENS_UP_TO_VERSIONS=1 a run counts also when its head
# commit holds this tree up to exactly that — rd_tree_same_but_versions of scripts/lib/verified.sh,
# the rule the release chain applies to its own records: every differing path documentation or a
# version file, and in those every changed line the workspace version, old for new. That needs the
# run's commit: it is fetched (`git fetch --depth=1 origin <sha>`) into the checkout
# CI_TREE_GREENS_CHECKOUT (the current directory), at most CI_TREE_GREENS_FETCHES (10) of them; a
# commit that cannot be fetched or compared does not count.
#
# Usage:
#   scripts/ci-tree-greens.sh <owner/repo> <tree sha> <image>... [-- <job>...]
#   scripts/ci-tree-greens.sh <owner/repo> <tree sha> -- <job>...
#
# Prints the images still to check as a JSON list for the matrices (`[]` when none), and with
# `--` a second line, the jobs still to run as a JSON list. Any answer GitHub does not give — no
# gh, no token, an API error — prints every image and every job: the safe side is the full run,
# never the skipped one. Looks at the newest CI_TREE_GREENS_RUNS successful runs (50).
# Needs `gh` with GH_TOKEN, and `actions: read` for the runs.
set -euo pipefail

usage() {
    echo "usage: $0 <owner/repo> <tree sha> <image>... [-- <job>...] | <owner/repo> <tree sha> -- <job>..." >&2
    exit 2
}
[[ $# -ge 3 ]] || usage
repo="$1"
tree="$2"
shift 2
images=()
named_jobs=()
with_jobs=0
while [[ $# -gt 0 ]]; do
    if [[ "$1" == -- ]]; then
        with_jobs=1
    elif [[ "$with_jobs" -eq 1 ]]; then
        named_jobs+=("$1")
    else
        images+=("$1")
    fi
    shift
done
[[ ${#images[@]} -gt 0 || ${#named_jobs[@]} -gt 0 ]] || usage
limit="${CI_TREE_GREENS_RUNS:-50}"
workflow="${CI_TREE_GREENS_WORKFLOW:-ci.yml}"
leg="${CI_TREE_GREENS_LEG:-rust}"
up_to_versions="${CI_TREE_GREENS_UP_TO_VERSIONS:-0}"
fetches="${CI_TREE_GREENS_FETCHES:-10}"
checkout="${CI_TREE_GREENS_CHECKOUT:-.}"
if [[ "$up_to_versions" == 1 ]]; then
    # shellcheck source=lib/verified.sh
    source "$(dirname "${BASH_SOURCE[0]}")/lib/verified.sh"
fi

json_list() {
    local out="" image
    for image in "$@"; do out+="${out:+,}\"$image\""; done
    printf '[%s]\n' "$out"
}

# Everything still to check, said once, when GitHub could not be asked.
all() {
    echo "::warning::ci-tree-greens: $1 — checking every image" >&2
    json_list "${images[@]+"${images[@]}"}"
    [[ "$with_jobs" -eq 0 ]] || json_list "${named_jobs[@]+"${named_jobs[@]}"}"
    exit 0
}

command -v gh > /dev/null || all "gh is not installed"
runs="$(gh api "repos/$repo/actions/workflows/$workflow/runs?status=success&per_page=$limit" \
    --jq '.workflow_runs[] | "\(.id) \(.head_sha)"' 2> /dev/null)" || all "the runs could not be listed"

declare -A green=()
declare -A green_job=()
declare -A trees=()
declare -A same=()
# Whether commit $1, whose tree is not this one, holds it up to documentation and a version bump.
same_but_versions() {
    local sha="$1"
    [[ "$up_to_versions" == 1 ]] || return 1
    if [[ -z "${same[$sha]:-}" ]]; then
        same[$sha]=no
        if [[ "$fetches" -gt 0 ]]; then
            fetches=$((fetches - 1))
            if git -C "$checkout" fetch -q --no-tags --depth=1 origin "$sha" > /dev/null 2>&1 \
                && rd_tree_same_but_versions "$checkout" "$sha" "$tree"; then
                same[$sha]=yes
                echo "run commit ${sha:0:12}: tree ${tree:0:12} up to documentation and versions" >&2
            fi
        fi
    fi
    [[ "${same[$sha]}" == yes ]]
}
while read -r run sha; do
    [[ -n "$run" ]] || continue
    if [[ -z "${trees[$sha]:-}" ]]; then
        trees[$sha]="$(gh api "repos/$repo/git/commits/$sha" --jq '.tree.sha' 2> /dev/null)" \
            || trees[$sha]="unknown"
    fi
    [[ "${trees[$sha]}" == "$tree" ]] || same_but_versions "$sha" || continue
    jobs="$(gh api "repos/$repo/actions/runs/$run/jobs?per_page=100" \
        --jq '.jobs[] | select(.conclusion == "success") | .name' 2> /dev/null)" || continue
    for image in "${images[@]+"${images[@]}"}"; do
        grep -qxF "$leg ($image)" <<< "$jobs" && green[$image]=1
    done
    while IFS= read -r name; do
        for job in "${named_jobs[@]+"${named_jobs[@]}"}"; do
            [[ "$name" == "$job" || "$name" == "$job ("* ]] && green_job[$job]=1
        done
    done <<< "$jobs"
done <<< "$runs"

missing=()
for image in "${images[@]+"${images[@]}"}"; do
    if [[ -n "${green[$image]:-}" ]]; then
        echo "${image}: green for tree ${tree:0:12}" >&2
    else
        missing+=("$image")
    fi
done
json_list "${missing[@]+"${missing[@]}"}"

[[ "$with_jobs" -eq 1 ]] || exit 0
missing=()
for job in "${named_jobs[@]+"${named_jobs[@]}"}"; do
    if [[ -n "${green_job[$job]:-}" ]]; then
        echo "${job}: green for tree ${tree:0:12}" >&2
    else
        missing+=("$job")
    fi
done
json_list "${missing[@]+"${missing[@]}"}"
