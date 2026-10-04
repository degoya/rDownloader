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
# Usage:
#   scripts/ci-tree-greens.sh <owner/repo> <tree sha> <image>... [-- <job>...]
#
# Prints the images still to check as a JSON list for the matrices (`[]` when none), and with
# `--` a second line, the jobs still to run as a JSON list. Any answer GitHub does not give — no
# gh, no token, an API error — prints every image and every job: the safe side is the full run,
# never the skipped one. Looks at the newest CI_TREE_GREENS_RUNS successful runs (50).
# Needs `gh` with GH_TOKEN, and `actions: read` for the runs.
set -euo pipefail

[[ $# -ge 3 ]] || { echo "usage: $0 <owner/repo> <tree sha> <image>... [-- <job>...]" >&2; exit 2; }
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
[[ ${#images[@]} -gt 0 ]] || { echo "usage: $0 <owner/repo> <tree sha> <image>... [-- <job>...]" >&2; exit 2; }
limit="${CI_TREE_GREENS_RUNS:-50}"

json_list() {
    local out="" image
    for image in "$@"; do out+="${out:+,}\"$image\""; done
    printf '[%s]\n' "$out"
}

# Everything still to check, said once, when GitHub could not be asked.
all() {
    echo "::warning::ci-tree-greens: $1 — checking every image" >&2
    json_list "${images[@]}"
    [[ "$with_jobs" -eq 0 ]] || json_list "${named_jobs[@]+"${named_jobs[@]}"}"
    exit 0
}

command -v gh > /dev/null || all "gh is not installed"
runs="$(gh api "repos/$repo/actions/workflows/ci.yml/runs?status=success&per_page=$limit" \
    --jq '.workflow_runs[] | "\(.id) \(.head_sha)"' 2> /dev/null)" || all "the runs could not be listed"

declare -A green=()
declare -A green_job=()
declare -A trees=()
while read -r run sha; do
    [[ -n "$run" ]] || continue
    if [[ -z "${trees[$sha]:-}" ]]; then
        trees[$sha]="$(gh api "repos/$repo/git/commits/$sha" --jq '.tree.sha' 2> /dev/null)" \
            || trees[$sha]="unknown"
    fi
    [[ "${trees[$sha]}" == "$tree" ]] || continue
    jobs="$(gh api "repos/$repo/actions/runs/$run/jobs?per_page=100" \
        --jq '.jobs[] | select(.conclusion == "success") | .name' 2> /dev/null)" || continue
    for image in "${images[@]}"; do
        grep -qxF "rust ($image)" <<< "$jobs" && green[$image]=1
    done
    while IFS= read -r name; do
        for job in "${named_jobs[@]+"${named_jobs[@]}"}"; do
            [[ "$name" == "$job" || "$name" == "$job ("* ]] && green_job[$job]=1
        done
    done <<< "$jobs"
done <<< "$runs"

missing=()
for image in "${images[@]}"; do
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
