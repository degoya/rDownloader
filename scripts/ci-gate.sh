#!/usr/bin/env bash
#
# The `gate` job of ci.yml (RD-191-09 T02): what this run has to check, as the step outputs
# `platforms`, `check`, `jobs` and `warm`. Everything, except on a push to `main`: a release pushes
# there the tree its chain already had checked as `ci/<version>` — one fresh commit per release,
# so the commit is new and the tree is not — and ci.yml ran it all again, macOS included, for 61
# minutes. scripts/ci-tree-greens.sh names the runner images no successful run of this very tree
# covers; the platform jobs take those, and with none left only `components` runs, whose cache
# release.yml reads. The jobs that run once per run, not per image (`ONCE`), are asked the same
# way and run only when no successful run of the tree passed them (RA-TOOL-05). The Linux and
# Windows images left out build for main's cache instead (`warm-cache`); macOS is checked at a
# release candidate only and keeps none. CI only.
#
#   REQUESTED='["ubuntu-24.04",…]' ONCE='["docker",…]' scripts/ci-gate.sh
#
# Reads GITHUB_EVENT_NAME, GITHUB_REF, GITHUB_REPOSITORY and GH_TOKEN as the runner sets them and
# appends to GITHUB_OUTPUT. Moved out of the workflow unchanged (RD-1101-07).
set -euo pipefail

platforms="${REQUESTED}"
jobs="${ONCE}"
warm='[]'
if [[ "${GITHUB_EVENT_NAME}" == push && "${GITHUB_REF}" == refs/heads/main ]]; then
    mapfile -t images < <(jq -r '.[]' <<< "${REQUESTED}")
    mapfile -t once < <(jq -r '.[]' <<< "${ONCE}")
    mapfile -t answer < <(scripts/ci-tree-greens.sh "${GITHUB_REPOSITORY}" "$(git rev-parse 'HEAD^{tree}')" "${images[@]}" -- "${once[@]}")
    platforms="${answer[0]:-${REQUESTED}}"
    jobs="${answer[1]:-${ONCE}}"
    # The Linux and Windows images left out above build for main's cache instead
    # (`warm-cache`); macOS is checked at a release candidate only and keeps none.
    warm="$(jq -c --argjson run "${platforms}" \
        '[.[] | select(startswith("macos") | not) | select(. as $image | $run | index($image) | not)]' \
        <<< "${REQUESTED}")"
fi
check=true
if [[ "${platforms}" == "[]" ]]; then check=false; fi
echo "platforms: ${platforms}"
echo "jobs: ${jobs}"
echo "warm: ${warm}"
{
    echo "platforms=${platforms}"
    echo "check=${check}"
    echo "jobs=${jobs}"
    echo "warm=${warm}"
} >> "${GITHUB_OUTPUT}"
