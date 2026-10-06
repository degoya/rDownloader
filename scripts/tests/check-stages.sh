#!/usr/bin/env bash
#
# scripts/lib/stages.sh, the stages of a check.sh run (RD-1100-13), without anything that
# compiles: a failing stage is recorded and the run goes on, the failure list names the stage,
# its exit status, its log and the failing tests and compiler errors in it (each once), a green
# stage leaves no trace in it, skips are listed with their reason, and the run then ends
# non-zero — or, with nothing failed, goes on. The script tests collect the same way
# (RD-1110-15): a red one is recorded and the next one runs.
#
#   scripts/tests/check-stages.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

# A stand-in for nextest: its FAIL lines twice, as a run prints them during the run and again in
# the summary, with timing and counter.
cat > "$SCRATCH/nextest" <<'EOF'
#!/usr/bin/env bash
echo "    Starting 3 tests across 1 binary"
echo "        PASS [   0.002s] (1/3) rd-core tests::fine"
echo "        FAIL [   0.004s] (2/3) rd-core tests::broken"
echo "     TIMEOUT [ 300.001s] (3/3) rd-core tests::hangs"
echo "     Summary [ 300.010s] 3 tests run: 1 passed, 2 failed"
echo "        FAIL [   0.004s] (2/3) rd-core tests::broken"
echo "     TIMEOUT [ 300.001s] (3/3) rd-core tests::hangs"
echo "error: test run failed" >&2
exit 100
EOF
cat > "$SCRATCH/clippy" <<'EOF'
#!/usr/bin/env bash
echo "error[E0425]: cannot find value \`x\` in this scope"
echo "  --> crates/rd-http/src/lib.rs:10:5"
echo "error: could not compile \`rd-http\` (lib) due to 1 previous error"
exit 101
EOF
chmod +x "$SCRATCH/nextest" "$SCRATCH/clippy"

# One run in a subshell of its own, as check.sh is one process: its output and status.
run() {
    run_status bash -c '
        set -euo pipefail
        CHECK_LOGS="$1" RUN_KIND=full
        source "$2/scripts/lib/stages.sh"
        rd_stages_init
        shift 2
        step "tests"
        attempt "$1"
        step "clippy"
        attempt "$2"
        step "fine"
        attempt true
        skip "sqlx offline data" "sqlx-cli is not installed"
        rd_stages_report
        rd_stages_exit_if_failed
        echo "==> all requested checks passed"
    ' run "$SCRATCH/logs" "$ROOT" "$@"
}

run "$SCRATCH/nextest" "$SCRATCH/clippy"
expect_status "a run with failed stages ends non-zero" 1
expect_output "after every stage ran" "==> fine"
expect_output "saying it goes on" "!! tests failed (exit 100); the run goes on"
expect_output "listing the skip with its reason" "- sqlx offline data — sqlx-cli is not installed"
expect_output "and the time of every stage" "total (all stages)"
expect_true "without the closing line" '! grep -q "all requested checks passed" <<< "$output"'
FAILURES="$SCRATCH/logs/failures"
expect "the failure list: two stages" "2" "$(grep -c ' — exit ' "$FAILURES")"
expect_true "each with its exit status and log" \
    'grep -qxF "tests — exit 100, log $SCRATCH/logs/full-01.log" "$FAILURES" && grep -qxF "clippy — exit 101, log $SCRATCH/logs/full-02.log" "$FAILURES"'
expect "a failing test once, without timing and counter" "1" "$(grep -cxF '    FAIL rd-core tests::broken' "$FAILURES")"
expect "a timed-out one too" "1" "$(grep -cxF '    TIMEOUT rd-core tests::hangs' "$FAILURES")"
expect_true "a compiler error with its location" \
    'grep -qF "    error[E0425]: cannot find value" "$FAILURES" && grep -qxF "      --> crates/rd-http/src/lib.rs:10:5" "$FAILURES"'
expect_true "a passing test is not in it" '! grep -q "tests::fine" "$FAILURES"'
expect_true "nor the green stage" '! grep -q "^fine" "$FAILURES"'
expect_true "each stage's whole output is in its log" 'grep -q "Summary" "$SCRATCH/logs/full-01.log"'

run true true
expect_status "a run without a failure goes on" 0
expect_output "to its closing line" "==> all requested checks passed"
expect_true "and an earlier run's failure list is gone" '[[ ! -e "$FAILURES" ]]'

# The script tests (RD-1110-15): each through `attempt`, so a red one no longer ends the run.
TESTS="$SCRATCH/checkout"
mkdir -p "$TESTS/scripts/tests"
printf '#!/usr/bin/env bash\necho "a is red"\nexit 3\n' > "$TESTS/scripts/tests/a.sh"
printf '#!/usr/bin/env bash\necho "b is green"\n' > "$TESTS/scripts/tests/b.sh"
chmod +x "$TESTS/scripts/tests/"*.sh
run_status bash -c '
    set -euo pipefail
    CHECK_LOGS="$1/logs" RUN_KIND=full
    source "$2/scripts/lib/stages.sh"
    source "$2/scripts/lib/script-checks.sh"
    rd_stages_init
    cd "$1"
    rd_script_tests
    rd_stages_report
    rd_stages_exit_if_failed
    echo "==> all requested checks passed"
' run "$TESTS" "$ROOT"
expect_status "a red script test fails the run" 1
expect_output "after the next test ran" "b is green"
expect_output "and the run went on to its report" "total (all stages)"
expect "the failure list names the red test, and only it" \
    "script test: scripts/tests/a.sh — exit 3, log $TESTS/logs/full-01.log" "$(grep ' — exit ' "$TESTS/logs/failures")"

# A --full over a tree a preflight green covers skips the script lints and tests (audit C2).
run_status bash -c '
    set -euo pipefail
    CHECK_LOGS="$1/logs" RUN_KIND=full full=1 changed="" preflight_covered=1
    source "$2/scripts/lib/stages.sh"
    source "$2/scripts/lib/script-checks.sh"
    rd_stages_init
    cd "$1"
    rd_script_checks
    rd_stages_report
' run "$TESTS" "$ROOT"
expect_output "a preflight green covers the script checks of a --full" \
    "- actionlint, bash -n, shellcheck and the script tests — a recorded preflight green covers this tree"
expect_true "and no test ran" '! grep -q "a is red" <<< "$output"'

# The lints record their half only when green (audit C8: --clippy-all as `clippy`, the gate's
# halves, --windows).
run_status bash -c '
    set -euo pipefail
    export CARGO_TARGET_DIR="$1/target"
    CHECK_LOGS="$1/logs" RUN_KIND=full ROOT="$1"
    source "$2/scripts/lib/stages.sh"
    source "$2/scripts/lib/verified.sh"
    source "$2/scripts/lib/lint.sh"
    rd_stages_init
    step "red"
    rd_lint_recorded clippy tree-red false
    step "green"
    rd_lint_recorded clippy tree-green true
    cat "$(rd_full_marker "$ROOT")"
' run "$TESTS" "$ROOT"
expect "a green lint records its half for the tree, a red one nothing" "clippy tree-green" \
    "$(grep '^clippy ' <<< "$output")"

finish_tests check-stages
