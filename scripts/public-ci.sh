#!/usr/bin/env bash
#
# Runs the public GitHub CI on a branch of this repository before it is merged (RD-140-22,
# RD-140-23): exports the branch to the public repository as ci/<branch>, starts the run, waits
# for it, and deletes the public branch on green. On red the branch stays, so the failed run can
# be read next to its tree — `scripts/ci-log.sh <run url>` prints what broke — and the next
# export of the same branch replaces it.
#
# Every wave's integration branch goes through this once, on Linux and Windows, before it is
# merged into development: v1.3.0 shipped with 42 Windows test failures that only GitHub's
# Windows runner could see. A red run holds the merge.
#
# Usage:
#   scripts/public-ci.sh integration/1.4-w4 --platforms linux,windows   # the wave's gate
#   scripts/public-ci.sh integration/1.4-w4                             # all three platforms
#   scripts/public-ci.sh fix/x --platforms windows-2025                 # a runner image by name
#
# Without --platforms the push starts ci.yml on every platform, as any push does. With it the
# export's commit carries `[skip ci]` and ci.yml is started by hand with its `platforms` input,
# so exactly one run is watched. Either way the export, the wait and the deletion are the ones
# the release pipeline's `public-ci` step uses (scripts/lib/public-ci.sh).
#
# Outward: the branch's tree becomes public while the run lasts, minus what
# scripts/public-exclude.txt leaves out. The export refuses anything gitleaks finds.
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/public-ci.sh
source "$ROOT/scripts/lib/public-ci.sh"
cd "$ROOT"

usage() { echo "usage: scripts/public-ci.sh <branch> [--platforms linux,windows,macos]" >&2; exit 2; }

branch=""
platforms=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --platforms) platforms="${2:?--platforms needs a list}"; shift 2 ;;
        -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
        -*) echo "unknown argument: $1" >&2; usage ;;
        *) [[ -z "$branch" ]] || usage; branch="$1"; shift ;;
    esac
done
[[ -n "$branch" ]] || usage

platforms_json=""
if [[ -n "$platforms" ]]; then
    platforms_json="$(rd_public_ci_platforms "$platforms")" || exit 2
fi
commit="$(git rev-parse --verify --quiet "$branch^{commit}")" \
    || { echo "no such branch or commit: $branch" >&2; exit 2; }
# The public branch: ci/ and the name with its slashes flattened, so integration/1.4-w4 becomes
# ci/integration-1.4-w4. The release pipeline's ci/<version> cannot collide with it.
public_branch="ci/${branch//\//-}"
# export-public.sh names the version in its commit message only; the branch's own workspace
# version is the honest one, without a pre-release suffix it does not accept.
version="$(git show "$commit:Cargo.toml" | sed -n '/^\[workspace\.package\]/,/^\[/p' \
    | sed -n 's/^version = "\([0-9]*\.[0-9]*\.[0-9]*\).*"/\1/p' | head -1)"
[[ -n "$version" ]] || { echo "no workspace version in $branch:Cargo.toml" >&2; exit 1; }

rd_public_ci_gh_ready
skip=()
[[ -z "$platforms_json" ]] || skip=(--skip-push-ci)
scripts/export-public.sh "$version" --ref "$commit" --branch "$public_branch" "${skip[@]}"
sha="$(git -C "$PUBLIC_DIR" rev-parse "refs/heads/$public_branch")"

event=""
if [[ -n "$platforms_json" ]]; then
    rd_public_ci_dispatch "$public_branch" "$platforms_json"
    event="workflow_dispatch"
fi
if ! rd_public_ci_wait "$public_branch" "$sha" "$event"; then
    echo "!! $branch is not green on GitHub; it is not merged until it is." >&2
    exit 1
fi
rd_public_ci_delete "$public_branch"
echo "==> $branch (${commit:0:12}) is green on GitHub${platforms:+ ($platforms)}"
