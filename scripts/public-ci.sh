#!/usr/bin/env bash
#
# Runs the public GitHub CI on a branch of this repository before it is merged (RD-140-22,
# RD-140-23): exports the branch to the public repository as ci/<branch>, starts the run, waits
# for it, and deletes the public branch on green. On red the branch stays, so the failed run can
# be read next to its tree — `scripts/ci-log.sh <run url>` prints what broke — and the next
# export of the same branch replaces it.
#
# Every wave's integration branch goes through this once, on Windows, before it is merged into
# development: v1.3.0 shipped with 42 Windows test failures that only GitHub's Windows runner could
# see. A red run holds the merge. Linux only when no local --full green of the Rust half covers the
# tree (RD-1280-14): scripts/integrate.sh --public-ci derives the list; the release candidate runs
# Linux here.
#
# Usage:
#   scripts/public-ci.sh integration/1.4-w4 --platforms windows         # the wave's gate
#   scripts/public-ci.sh integration/1.4-w4 --platforms linux,windows   # ... without a local green
#   scripts/public-ci.sh integration/1.4-w4                             # all three platforms
#   scripts/public-ci.sh fix/x --platforms windows-2025                 # a runner image by name
#   scripts/public-ci.sh integration/1.4-w4 --platforms linux,windows --release-workflows
#
# --release-workflows (RD-1270-10) dispatches beside ci.yml the release workflows
# (RD_PUBLIC_CI_RELEASE_WORKFLOWS: e2e, recovery, self-update, installers, each with its defaults)
# that no green covers, waits for them with it and records their greens per file name and tree:
# `integrate.sh --public-ci` passes it, so the release candidate finds them on record and runs
# only what is left. A red job or run ends the wait at once, naming it.
#
# Not twice (RD-160-06): a platform already green for the branch's tree, or for a tree that
# differs from it only in documentation and version lines, is not run again; with every named
# platform green nothing is exported at all. The export's commit carries `[skip ci]` and ci.yml
# is started by hand with its `platforms` input for the rest, so exactly one run is watched (and one
# per release workflow with --release-workflows); its green is recorded per platform. With Linux and Windows green on record its `jobs` input is `[]`:
# the once-per-run jobs passed with them (RD-1120-07). The plan, the export, the wait, the record and the deletion
# are the ones the release pipeline's `public-ci` step uses (scripts/lib/public-ci.sh).
#
# Outward: the branch's tree becomes public while the run lasts, minus what
# scripts/public-exclude.txt leaves out. The export refuses anything gitleaks finds.
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/public-ci.sh
source "$ROOT/scripts/lib/public-ci.sh"
# shellcheck source=lib/workspace-version.sh
source "$ROOT/scripts/lib/workspace-version.sh"
cd "$ROOT"

usage() { echo "usage: scripts/public-ci.sh <branch> [--platforms linux,windows,macos] [--release-workflows]" >&2; exit 2; }

branch=""
platforms=""
release_workflows=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --platforms) platforms="${2:?--platforms needs a list}"; shift 2 ;;
        --release-workflows) release_workflows=1; shift ;;
        -h|--help) sed -n '2,37p' "$0"; exit 0 ;;
        -*) echo "unknown argument: $1" >&2; usage ;;
        *) [[ -z "$branch" ]] || usage; branch="$1"; shift ;;
    esac
done
[[ -n "$branch" ]] || usage

image_list="$(rd_public_ci_images "${platforms:-$RD_PUBLIC_CI_ALL}")" || exit 2
mapfile -t images <<< "$image_list"
commit="$(git rev-parse --verify --quiet "$branch^{commit}")" \
    || { echo "no such branch or commit: $branch" >&2; exit 2; }
tree="$(git rev-parse "$commit^{tree}")"
# The public branch: ci/ and the name with its slashes flattened, so integration/1.4-w4 becomes
# ci/integration-1.4-w4. The release pipeline's ci/<version> cannot collide with it.
public_branch="ci/${branch//\//-}"
# export-public.sh names the version in its commit message only; the branch's own workspace
# version is the honest one, without a pre-release suffix it does not accept.
version="$(git show "$commit:Cargo.toml" | rd_workspace_version)"
version="${version%%[-+]*}"
[[ -n "$version" ]] || { echo "no workspace version in $branch:Cargo.toml" >&2; exit 1; }

rd_public_ci_log_opening "$branch" "$commit" "$tree"
checked=("${images[@]}")
workflows=()
if [[ "$release_workflows" -eq 1 ]]; then
    checked+=("${RD_PUBLIC_CI_RELEASE_WORKFLOWS[@]}")
    rd_public_ci_plan "$ROOT" "$tree" "${RD_PUBLIC_CI_RELEASE_WORKFLOWS[@]}"
    workflows=(${RD_PUBLIC_CI_MISSING[@]+"${RD_PUBLIC_CI_MISSING[@]}"})
fi
rd_public_ci_plan "$ROOT" "$tree" "${images[@]}"
missing=(${RD_PUBLIC_CI_MISSING[@]+"${RD_PUBLIC_CI_MISSING[@]}"} ${workflows[@]+"${workflows[@]}"})
if [[ ${#missing[@]} -eq 0 ]]; then
    rd_public_ci_log_green "$branch" "$commit" "${checked[*]}" " already; nothing was run"
    exit 0
fi

rd_public_ci_gh_ready
scripts/export-public.sh "$version" --ref "$commit" --branch "$public_branch" --skip-push-ci
sha="$(git -C "$PUBLIC_DIR" rev-parse "refs/heads/$public_branch")"
# What an earlier run that ended during the wait left green on the branch counts (rd_public_ci_adopt).
public_tree="$(git -C "$PUBLIC_DIR" rev-parse "$sha^{tree}")"
rd_public_ci_adopt "$ROOT" "$tree" "$public_branch" "$public_tree" \
    ${RD_PUBLIC_CI_MISSING[@]+"${RD_PUBLIC_CI_MISSING[@]}"}
RD_PUBLIC_CI_MISSING=(${RD_PUBLIC_CI_LEFT[@]+"${RD_PUBLIC_CI_LEFT[@]}"})
rd_public_ci_adopt "$ROOT" "$tree" "$public_branch" "$public_tree" ${workflows[@]+"${workflows[@]}"}
workflows=(${RD_PUBLIC_CI_LEFT[@]+"${RD_PUBLIC_CI_LEFT[@]}"})
missing=(${RD_PUBLIC_CI_MISSING[@]+"${RD_PUBLIC_CI_MISSING[@]}"} ${workflows[@]+"${workflows[@]}"})
if [[ ${#missing[@]} -eq 0 ]]; then
    rd_public_ci_delete "$public_branch"
    rd_public_ci_log_green "$branch" "$commit" "${checked[*]}" " already; nothing was run"
    exit 0
fi
platforms_json=""
if [[ ${#RD_PUBLIC_CI_MISSING[@]} -gt 0 ]]; then
    platforms_json="$(rd_public_ci_platforms "$(IFS=,; echo "${RD_PUBLIC_CI_MISSING[*]}")")"
fi
if [[ -n "$platforms_json" ]]; then
    rd_public_ci_dispatch "$public_branch" "$platforms_json" "$(rd_public_ci_once_jobs "$ROOT" "$tree")"
fi
for workflow in ${workflows[@]+"${workflows[@]}"}; do
    rd_public_ci_dispatch_workflow "$public_branch" "$workflow"
done
if ! rd_public_ci_wait "$public_branch" "$sha" workflow_dispatch; then
    echo "!! $branch is not green on GitHub; it is not merged until it is." >&2
    exit 1
fi
rd_record_ci "$ROOT" "$tree" "${missing[@]}"
rd_public_ci_delete "$public_branch"
rd_public_ci_log_green "$branch" "$commit" "${checked[*]}" "; recorded for ${missing[*]}"
