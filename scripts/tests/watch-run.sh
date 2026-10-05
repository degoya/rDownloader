#!/usr/bin/env bash
#
# scripts/watch-run.sh against a stand-in run (RD-1100-13): it prints stage starts, failures and
# the closing lines and nothing else, follows a second log that appears only later, returns once
# the process has ended, and judges each log by its closing line.
#
#   scripts/tests/watch-run.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

# A run: noise and a stage into one log, then a second log with a failing test and a green end.
(
    sleep 0.3
    printf '%s\n' "noise" "==> the Windows lint" "    Checking rd-core" "==> all requested checks passed" \
        "REAL EXIT: 0" > "$SCRATCH/windows.log"
    sleep 0.3
    printf '%s\n' "==> tests (everything)" "        PASS [   0.002s] (1/2) rd-core tests::fine" \
        "        FAIL [   0.004s] (2/2) rd-core tests::broken" "!! tests failed (exit 100)" \
        "REAL EXIT: 1" > "$SCRATCH/check.log"
) &
run_pid=$!

run_status timeout 30 "$ROOT/scripts/watch-run.sh" "$run_pid" "$SCRATCH/windows.log" "$SCRATCH/check.log"
expect_status "a log that does not end green fails the watch" 1
expect_output "a stage start" "==> the Windows lint"
expect_output "the second log, written later" "==> tests (everything)"
expect_output "a failing test" "FAIL [   0.004s] (2/2) rd-core tests::broken"
expect_output "a failure line" "!! tests failed (exit 100)"
expect_true "no noise" '! grep -qE "noise|Checking|PASS" <<< "$output"'
expect_output "the verdict per log: green" "windows.log: green (exit 0)"
expect_output "and not green, with its exit" "check.log: NOT green (exit 1)"

printf '%s\n' "==> all requested checks passed" > "$SCRATCH/check.log"
run_status timeout 30 "$ROOT/scripts/watch-run.sh" "$run_pid" "$SCRATCH/windows.log" "$SCRATCH/check.log"
expect_status "every log green, the process gone: passes at once" 0

run_status "$ROOT/scripts/watch-run.sh" notapid "$SCRATCH/check.log"
expect_status "a pid that is not one is a usage error" 2

finish_tests watch-run
