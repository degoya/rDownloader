#!/usr/bin/env bash
#
# The production scripts no other test reached (RD-1280-15): each parses (`bash -n`) and answers a
# call that does nothing — no arguments, an unknown one or `--help` — with its usage and exit 2
# (exit 0 for `--help`), before it reads a secret, builds, downloads or touches the checkout. Most
# of them run only on a release or a CI runner, where a syntax error or a broken argument parser
# showed up for the first time; this is the floor under them, not a test of what they do.
#
#   scripts/tests/script-usage.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

# <script> <expected exit> <text the answer carries> [argument...]
usage_of() {
    local script="$1" want="$2" needle="$3"
    shift 3
    run_status bash -n "$ROOT/scripts/$script"
    expect_status "$script parses" 0
    # No secret, runner or release variable reaches the call: a script that needs one before it
    # reads its arguments fails here instead of on the runner.
    run_status env -i PATH="$PATH" HOME="$HOME" "$ROOT/scripts/$script" "$@"
    expect_status "$script ${*:-(no arguments)} exits $want" "$want"
    expect_output "$script ${*:-(no arguments)} says so" "$needle"
}

usage_of check-capture-linux-tree.sh 0 "Usage:" --help
usage_of check-plugin-imports.sh 2 "usage: scripts/check-plugin-imports.sh"
usage_of ci-platform-smoke.sh 2 "usage: scripts/ci-platform-smoke.sh"
usage_of ci-package-repo.sh 2 "usage: scripts/ci-package-repo.sh"
usage_of ci-package-repo.sh 2 "usage: scripts/ci-package-repo.sh" apt remove
usage_of ci-plugins.sh 2 "usage: scripts/ci-plugins.sh"
usage_of ci-services.sh 2 "usage: scripts/ci-services.sh"
usage_of ci-tools.sh 2 "usage: scripts/ci-tools.sh"
usage_of dev.sh 2 "unknown argument: --no-such-option" --no-such-option
usage_of docker.sh 2 "usage: scripts/docker.sh"
usage_of generate-icons.sh 0 "favicon.svg" --help
usage_of generate-icons.sh 2 "usage: scripts/generate-icons.sh" extra
usage_of owner-site-rules.sh 2 "unknown argument: --no-such-option" --no-such-option
usage_of owner-site-rules.sh 0 "Usage:" --help
usage_of release-binary.sh 2 "usage: scripts/release-binary.sh"
usage_of release-channels.sh 2 "usage: scripts/release-channels.sh"
usage_of release-packages.sh 2 "usage: scripts/release-packages.sh"
usage_of release-repositories.sh 2 "usage: scripts/release-repositories.sh"
usage_of release-smoke.sh 2 "usage: scripts/release-smoke.sh"
usage_of release-start.sh 2 "usage: scripts/release-start.sh"
usage_of release-start.sh 0 "Usage:" --help
usage_of self-update-smoke.sh 2 "usage: scripts/self-update-smoke.sh"
usage_of tag-release.sh 0 "Usage:" --help
usage_of tag-release.sh 2 "usage: scripts/tag-release.sh" --no-such-option

finish_tests "script-usage"
