#!/usr/bin/env bash
#
# scripts/e2e.sh's build half (RD-191-09): `--build` re-runs the script as `--build-only`, and that
# call builds through cargo with the job cap of scripts/lib/jobs.sh — `-j` and CARGO_BUILD_JOBS —
# in the checkout's target directory. A stand-in cargo records what it was asked; the lock is
# switched off (RD_NO_LOCK=1), since scripts/tests/lock-lanes.sh tests it.
#
# Pure bash, no cargo. check.sh runs it when scripts/ change, and under --full.
#
#   scripts/tests/e2e-build.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

mkdir -p "$SCRATCH/bin"
cat > "$SCRATCH/bin/cargo" <<'FAKE'
#!/usr/bin/env bash
printf 'CARGO_BUILD_JOBS=%s CARGO_TARGET_DIR=%s %s\n' "${CARGO_BUILD_JOBS:-}" "${CARGO_TARGET_DIR:-}" "$*" \
    >> "$RD_TEST_CALLS"
FAKE
chmod +x "$SCRATCH/bin/cargo"
export RD_TEST_CALLS="$SCRATCH/calls"

run_status env PATH="$SCRATCH/bin:$PATH" RD_NO_LOCK=1 JOBS=3 CARGO_TARGET_DIR="$SCRATCH/target" \
    "$ROOT/scripts/e2e.sh" --build-only
expect_status "--build-only builds and exits 0" 0
expect "one cargo call, capped at JOBS, in the target directory" \
    "CARGO_BUILD_JOBS=3 CARGO_TARGET_DIR=$SCRATCH/target build --locked --profile release-test -j 3 -p rdownloader -p rd-capture" \
    "$(cat "$SCRATCH/calls")"

rm -f "$SCRATCH/calls"
run_status env PATH="$SCRATCH/bin:$PATH" RD_NO_LOCK=1 JOBS=0 "$ROOT/scripts/e2e.sh" --build-only
expect_status "a JOBS that is not a positive number is refused" 2
expect_true "and nothing was built" '[[ ! -e "$SCRATCH/calls" ]]'

finish_tests "e2e-build"
