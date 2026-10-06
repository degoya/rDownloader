#!/usr/bin/env bash
#
# The `gate` job of channels.yml (RD-180-06; moved out of the workflow in RD-1120-07): whether and
# against what the package channels are checked, as the step outputs `run`, `expect` and `from`.
#
#  * A dispatch takes its inputs `expect_version` and `upgrade_from`, each refused unless it is a
#    plain version or release tag.
#  * A Release run of a plain `vX.Y.Z` tag that succeeded checks that the channels carry its
#    version and upgrades to it from the plain release before it (RD-1120-07): until 1.12 the
#    release ran the fresh install only, and no hand dispatch with `upgrade_from` ever happened, so
#    the upgrade with data — the point of jobs RD-180-06 and RD-180-10 — was never run. Without an
#    earlier release, or when GitHub does not name one, the fresh install is checked and a warning
#    says why.
#  * Any other Release run (failed, a beta) moves no channel and checks nothing.
#
# CI only. Reads EVENT, CONCLUSION, RUN_REF, EXPECT, FROM and GITHUB_REPOSITORY as channels.yml
# sets them, GH_TOKEN for `gh`, and appends to GITHUB_OUTPUT.
set -euo pipefail

plain_tag='^v[0-9]+\.[0-9]+\.[0-9]+$'
outputs() { printf '%s\n' "$@" >> "${GITHUB_OUTPUT}"; }

if [[ "${EVENT}" == workflow_dispatch ]]; then
    if [[ -n "${FROM:-}" && ! "${FROM}" =~ $plain_tag ]]; then
        echo "::error::upgrade_from must be a plain release tag like v1.6.1, not '${FROM}'"
        exit 1
    fi
    if [[ -n "${EXPECT:-}" && ! "${EXPECT}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        echo "::error::expect_version must be a version like 1.7.0, not '${EXPECT}'"
        exit 1
    fi
    outputs "run=true" "expect=${EXPECT:-}" "from=${FROM:-}"
    exit 0
fi

# A tag-triggered run names its tag in head_branch.
if [[ "${CONCLUSION:-}" != success || ! "${RUN_REF:-}" =~ $plain_tag ]]; then
    echo "::notice::Release run of '${RUN_REF:-}' (${CONCLUSION:-}) does not move the channels; nothing to check"
    outputs "run=false"
    exit 0
fi

# The newest plain release below this one: GitHub's list, the tag itself added, sorted by version.
previous=""
if tags="$(gh release list --repo "${GITHUB_REPOSITORY}" --exclude-drafts --exclude-pre-releases \
    --limit 100 --json tagName --jq '.[].tagName' 2> /dev/null)"; then
    previous="$({ grep -E "$plain_tag" <<< "${tags}" || true; echo "${RUN_REF}"; } \
        | sort -V -u | grep -B 1 -x -F -- "${RUN_REF}" | head -n 1)"
    [[ "${previous}" != "${RUN_REF}" ]] || previous=""
fi
if [[ -z "${previous}" ]]; then
    echo "::warning::no plain release before ${RUN_REF} found; the channels are checked by a fresh install only"
else
    echo "upgrading from ${previous} to ${RUN_REF}"
fi
outputs "run=true" "expect=${RUN_REF#v}" "from=${previous}"
