#!/usr/bin/env bash
#
# scripts/watch-run.sh against a stand-in run (RD-1100-13): it prints stage starts, failures and
# the closing lines and nothing else, follows a second log that appears only later, returns once
# the process has ended, and judges each log by its kind's closing line: check.sh's, public-ci.sh's
# own green (RD-1300-03), any other log's REAL EXIT.
#
#   scripts/tests/watch-run.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
# shellcheck source=../lib/public-ci.sh
source "$ROOT/scripts/lib/public-ci.sh"

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

# --- public-ci.log, its lines from the library public-ci.sh prints them with ----------------------
commit=0123456789abcdef0123 tree=fedcba98765432100123
{
    rd_public_ci_log_opening integration/1.30-w1 "$commit" "$tree"
    echo "  12:00:00 run CI (https://example.invalid/runs/1): completed success"
    echo "the public CI is green on ${commit:0:12}"
    rd_public_ci_log_green integration/1.30-w1 "$commit" "windows-2025 e2e.yml" "; recorded for windows-2025"
    echo "==> platforms derived from the recorded green: windows — the local Rust green covers this tree"
    echo "REAL EXIT: 0"
} > "$SCRATCH/public-ci.log"
run_status timeout 30 "$ROOT/scripts/watch-run.sh" "$run_pid" "$SCRATCH/windows.log" "$SCRATCH/check.log" \
    "$SCRATCH/public-ci.log"
expect_status "a green public-ci.log beside green check logs passes" 0
expect_output "read by its own green line" "public-ci.log: green on GitHub (exit 0)"
expect_output "check.log still by check.sh's closing line" "check.log: green"

# integrate.sh's second run, Linux beside Windows, red after the first one's green.
{
    rd_public_ci_log_opening integration/1.30-w1 "$commit" "$tree"
    echo "!! integration/1.30-w1 is not green on GitHub; it is not merged until it is."
    echo "REAL EXIT: 1"
} >> "$SCRATCH/public-ci.log"
run_status timeout 30 "$ROOT/scripts/watch-run.sh" "$run_pid" "$SCRATCH/public-ci.log"
expect_status "a red second run fails the watch" 1
expect_output "the first run's green does not count for it" "public-ci.log: NOT green on GitHub (exit 1)"

{ rd_public_ci_log_opening integration/1.30-w1 "$commit" "$tree"; echo "REAL EXIT: 2"; } > "$SCRATCH/public-ci.log"
run_status timeout 30 "$ROOT/scripts/watch-run.sh" "$run_pid" "$SCRATCH/public-ci.log"
expect_status "a public-ci run ended without a verdict line fails" 1
expect_output "with its exit" "public-ci.log: NOT green on GitHub (exit 2)"

# --- a log of no known kind: its REAL EXIT, said so ---------------------------------------------
printf '%s\n' "prune: 3 GB freed" "REAL EXIT: 0" > "$SCRATCH/prune.log"
run_status timeout 30 "$ROOT/scripts/watch-run.sh" "$run_pid" "$SCRATCH/prune.log"
expect_status "an unknown log with REAL EXIT 0 passes" 0
expect_output "saying what it was judged by" "prune.log: green by its REAL EXIT alone, no closing line of a known kind (exit 0)"
printf '%s\n' "prune: failed" > "$SCRATCH/prune.log"
run_status timeout 30 "$ROOT/scripts/watch-run.sh" "$run_pid" "$SCRATCH/prune.log"
expect_status "an unknown log without REAL EXIT fails" 1
expect_output "saying so" "prune.log: NOT green, no closing line of a known kind (exit none written)"

run_status "$ROOT/scripts/watch-run.sh" notapid "$SCRATCH/check.log"
expect_status "a pid that is not one is a usage error" 2

finish_tests watch-run
