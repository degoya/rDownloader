#!/usr/bin/env bash
#
# The whole release chain in the order it has to happen.
#
# Each step is its own script and can be run alone; this exists so the order and the "did the
# checks pass first" part are not reconstructed from memory each time.
#
# Usage:
#   scripts/release.sh                 # build and package the current version
#   scripts/release.sh 0.9.3           # bump to 0.9.3 first
#   scripts/release.sh --no-checks     # skip the test run (packaging only)
#   scripts/release.sh --plugins       # rebuild and re-sign the bundled plugins too
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Sourced before the `cd`, because the lock library resolves this script's own path from $0.
# shellcheck source=lib/lock.sh
source "$ROOT/scripts/lib/lock.sh"
rd_take_lock "$@"
cd "$ROOT"

run_checks=1
build_plugins=0
version=""
for argument in "$@"; do
    case "$argument" in
        --no-checks) run_checks=0 ;;
        --plugins) build_plugins=1 ;;
        -*) echo "unknown argument: $argument" >&2; exit 2 ;;
        *) version="$argument" ;;
    esac
done

if [[ -n "$version" ]]; then
    scripts/set-version.sh "$version"
    # The packages below are then the release, built before its commit exists: VERSION.txt names
    # it instead of `<commit>-dirty` (scripts/lib/version-file.sh), as release-pipeline.sh does.
    export RD_RELEASE_VERSION="$version"
fi
current="$(scripts/set-version.sh)"
echo "==> releasing $current"

# --full: package-windows.sh below refuses a tree without a --full green (RD-120-58).
if [[ "$run_checks" -eq 1 ]]; then
    scripts/check.sh --full
fi

# Only when a plugin actually changed: signing needs the release key, and re-packaging an
# unchanged plugin produces a new file for no reason.
if [[ "$build_plugins" -eq 1 ]]; then
    scripts/build-plugins.sh
fi

# Always materialize the browser packages in artifacts/, including with --no-checks. A regular
# check already tested them; rebuilding here is cheap and makes the release output deterministic.
scripts/build-extension.sh --skip-tests

# Linux first: it builds web/dist, which the Windows run then reuses.
scripts/package-linux.sh
scripts/package-windows.sh --skip-web

cat <<SUMMARY

==> $current and the browser extensions are packaged in artifacts/

    Still by hand, because they are judgement rather than mechanics:
      - CHANGELOG.md, docs/roadmap.md and the job files
      - committing the result
      - scripts/tag-release.sh, once that commit exists
SUMMARY
