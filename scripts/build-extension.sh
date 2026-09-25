#!/usr/bin/env bash
#
# Tests and builds the dependency-free browser extension for Chrome and Firefox. All generated
# files go below artifacts/browser-extensions, alongside the other release build outputs.
#
# Usage:
#   scripts/build-extension.sh               # tests, then builds both targets
#   scripts/build-extension.sh --skip-tests  # builds only
#   scripts/build-extension.sh --test-only   # tests only
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

mode="all"
for argument in "$@"; do
    case "$argument" in
        --skip-tests)
            [[ "$mode" == "all" ]] || { echo "--skip-tests and --test-only are mutually exclusive" >&2; exit 2; }
            mode="build"
            ;;
        --test-only)
            [[ "$mode" == "all" ]] || { echo "--skip-tests and --test-only are mutually exclusive" >&2; exit 2; }
            mode="test"
            ;;
        *) echo "unknown argument: $argument" >&2; exit 2 ;;
    esac
done

if ! command -v node > /dev/null; then
    echo "node is required to test and build the browser extension" >&2
    exit 1
fi

if [[ "$mode" != "test" ]]; then
    for tool in zip unzip; do
        command -v "$tool" > /dev/null && continue
        # The archives are what a release publishes and what a store accepts; a build that can
        # only leave the unpacked directories behind has not produced the artifact (RD-109-17).
        echo "$tool is required to package and verify the browser extension" >&2
        exit 1
    done
fi

if [[ "$mode" != "build" ]]; then
    echo "==> testing the browser extension"
    node --test extension/test/*.test.mjs
fi

if [[ "$mode" != "test" ]]; then
    echo "==> building the browser extension for Chrome and Firefox"
    node extension/build.mjs

    echo "==> verifying the packaged archives"
    expected="$(sed -n '/^\[workspace\.package\]/,/^\[/p' Cargo.toml \
        | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)"
    expected="${expected%%[-+]*}"
    for target in chrome firefox; do
        archive="artifacts/browser-extensions/rdownloader-$target.zip"
        [[ -f "$archive" ]] || { echo "$archive was not written" >&2; exit 1; }
        # Both halves are silenced so the failure reads as one sentence rather than as an
        # unzip caution followed by a Python traceback.
        packaged="$(unzip -p "$archive" manifest.json 2> /dev/null \
            | python3 -c 'import json,sys;print(json.load(sys.stdin)["version"])' 2> /dev/null)" \
            || { echo "$archive carries no readable manifest.json" >&2; exit 1; }
        [[ "$packaged" == "$expected" ]] \
            || { echo "$archive ships version $packaged, the workspace is at $expected" >&2; exit 1; }
        echo "    $archive: manifest.json, version $packaged"
    done
fi
