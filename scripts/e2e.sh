#!/usr/bin/env bash
#
# The end-to-end runs of RD-180-12 on this machine, the same files `.github/workflows/e2e.yml`
# runs: the Chrome extension in Chromium against a fresh service (web/e2e/extension.e2e.mjs) and
# the capture agent against one (web/e2e/capture.e2e.mjs). Both start their own throwaway service;
# nothing here touches a running installation.
#
# Usage:
#   scripts/e2e.sh                     # both, with the newest binaries found (see below)
#   scripts/e2e.sh --browser           # the extension only
#   scripts/e2e.sh --capture           # the capture agent only
#   scripts/e2e.sh --bin-dir DIR       # rdownloader and rdownloader-capture from DIR
#   scripts/e2e.sh --build             # build both in `release-test` first
#
# Without --bin-dir the binaries come from whichever of the target directory's `release-test`,
# its `release` and artifacts/linux holds the newest pair; the run prints which one and its
# version. Logs go to RD_E2E_LOG_DIR (default /tmp/claude-<uid>/e2e/<time>).
# Lock-free: only --build takes the target's build lock, around the bare cargo call.
#
# The extension run needs 127.0.0.1:8710 free (the one address its manifest grants at install)
# and Playwright's Chromium (`pnpm --dir web exec playwright install chromium`, done here). The
# capture run keeps the agent in a throwaway profile and skips the autostart step, which writes a
# real login entry; CI runs it with RD_E2E_OS_INTEGRATION=1 on disposable runners.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
# shellcheck source=scripts/lib/lanes.sh
source "$ROOT/scripts/lib/lanes.sh"

browser=0
capture=0
build=0
bin_dir="${RD_E2E_BIN_DIR:-}"
while [[ $# -gt 0 ]]; do
    case "$1" in
        --browser) browser=1 ;;
        --capture) capture=1 ;;
        --build) build=1 ;;
        --bin-dir)
            [[ $# -ge 2 ]] || { echo "--bin-dir needs a directory" >&2; exit 2; }
            bin_dir="$2"
            shift
            ;;
        -h | --help)
            sed -n '2,23p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
    shift
done
if [[ "$browser" -eq 0 && "$capture" -eq 0 ]]; then
    browser=1
    capture=1
fi

target="$(rd_target_dir "$ROOT")"
if [[ "$build" -eq 1 ]]; then
    [[ -z "$bin_dir" ]] || { echo "--build and --bin-dir are mutually exclusive" >&2; exit 2; }
    [[ -f web/dist/index.html ]] \
        || { echo "web/dist is missing; rust-embed needs the built frontend (pnpm --dir web run build)" >&2; exit 1; }
    lock="$(rd_target_lock_file "$target" "$ROOT")"
    echo "==> building rdownloader and rdownloader-capture (release-test) under $lock"
    # A shared target links other checkouts' rlibs unless this checkout's sources are newer.
    CARGO_TARGET_DIR="$target" flock -o "$lock" bash -c \
        'find crates -name "*.rs" -exec touch {} + && cargo build --locked --profile release-test -p rdownloader -p rd-capture'
    bin_dir="$target/release-test"
fi

# The newest of the three, not the first: a `release-test` build from weeks ago must not outvote
# the package built this morning.
if [[ -z "$bin_dir" ]]; then
    for candidate in "$target/release-test" "$target/release" "$ROOT/artifacts/linux"; do
        [[ -x "$candidate/rdownloader" && -x "$candidate/rdownloader-capture" ]] || continue
        if [[ -z "$bin_dir" || "$candidate/rdownloader" -nt "$bin_dir/rdownloader" ]]; then
            bin_dir="$candidate"
        fi
    done
fi
[[ -n "$bin_dir" && -x "$bin_dir/rdownloader" ]] \
    || { echo "no rdownloader binary found; pass --bin-dir or --build" >&2; exit 1; }
echo "==> binaries from $bin_dir ($("$bin_dir/rdownloader" --version))"

work="${RD_E2E_LOG_DIR:-/tmp/claude-$(id -u)/e2e/$(date +%Y%m%d-%H%M%S)}"
mkdir -p "$work"
echo "==> logs under $work"
failures=0

if [[ "$browser" -eq 1 ]]; then
    scripts/build-extension.sh --skip-tests
    pnpm --dir web exec playwright install chromium
    echo "==> the Chrome extension against a fresh service"
    if RD_E2E_SERVER="$bin_dir/rdownloader" RD_E2E_EXTENSION="$ROOT/artifacts/browser-extensions/chrome" \
       RD_E2E_WORK="$work/extension" node --test --test-reporter=spec web/e2e/extension.e2e.mjs; then
        echo "ok   the extension run"
    else
        echo "FAIL the extension run (service log: $work/extension/service/service.log)" >&2
        failures=$((failures + 1))
    fi
fi

if [[ "$capture" -eq 1 ]]; then
    [[ -x "$bin_dir/rdownloader-capture" ]] || { echo "no rdownloader-capture in $bin_dir" >&2; exit 1; }
    echo "==> the capture agent against a fresh service"
    if RD_E2E_SERVER="$bin_dir/rdownloader" RD_E2E_CAPTURE="$bin_dir/rdownloader-capture" \
       RD_E2E_WORK="$work/capture" node --test --test-reporter=spec web/e2e/capture.e2e.mjs; then
        echo "ok   the capture run"
    else
        echo "FAIL the capture run (logs: $work/capture)" >&2
        failures=$((failures + 1))
    fi
fi

echo
if [[ "$failures" -eq 0 ]]; then
    echo "==> all requested checks passed"
else
    echo "==> $failures end-to-end run(s) failed" >&2
fi
exit "$failures"
