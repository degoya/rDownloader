#!/usr/bin/env bash
#
# scripts/lib/junit-flaky.py (RD-191-09 T15) on scratch JUnit reports in nextest's shape: a test
# that passed on its retry becomes one `::warning` naming test, platform and run; a pass, a plain
# failure and a test that failed every attempt do not; an unreadable report is a notice, and the
# exit is 0 throughout.
#
# Pure python3 and bash. check.sh runs it when scripts/ change, and under --full.
#
#   scripts/tests/junit-flaky.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

cat > "$SCRATCH/group-1.xml" <<'EOF2'
<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="4" failures="1" errors="0">
  <testsuite name="rd-scheduler::control" tests="4" failures="1" errors="0">
    <testcase name="steady" classname="rd-scheduler::control" time="0.1"/>
    <testcase name="tick_races_the_pause" classname="rd-scheduler::control" time="2.0">
      <flakyFailure message="assertion failed: paused" type="test failure">thread panicked
at control.rs:849</flakyFailure>
    </testcase>
    <testcase name="always_broken" classname="rd-scheduler::control" time="0.2">
      <failure message="assertion failed" type="test failure"/>
      <rerunFailure message="assertion failed" type="test failure"/>
    </testcase>
    <testcase name="plain_failure" classname="rd-scheduler::control" time="0.2">
      <failure type="test failure"/>
    </testcase>
  </testsuite>
</testsuites>
EOF2
cat > "$SCRATCH/group-2.xml" <<'EOF2'
<testsuites><testsuite name="rd-http"><testcase name="resume_100%" classname="rd-http::resume">
<flakyError type="test abort"/><flakyFailure message="timed out"/></testcase></testsuite></testsuites>
EOF2
printf 'not xml' > "$SCRATCH/broken.xml"

run_status env RUNNER_OS=Windows GITHUB_RUN_ID=4242 GITHUB_RUN_ATTEMPT=2 \
    python3 "$ROOT/scripts/lib/junit-flaky.py" "$SCRATCH/group-1.xml" "$SCRATCH/group-2.xml" \
    "$SCRATCH/broken.xml" "$SCRATCH/missing.xml"
expect_status "flaky tests never fail the step" 0
expect_output "a test that passed on its retry is a warning with platform and run" \
    "::warning title=Flaky test (Windows)::rd-scheduler::control tick_races_the_pause failed 1x and passed on retry, on Windows, run 4242 attempt 2 — first failure: assertion failed: paused"
expect_output "every flaky attempt counts, and % is escaped" \
    "rd-http::resume resume_100%25 failed 2x and passed on retry"
expect_true "a pass and a failure, retried or not, are no warning" \
    '! grep -qE "steady|always_broken|plain_failure" <<< "$output"'
expect_output "an unreadable report is a notice" "::notice title=JUnit report not read::$SCRATCH/broken.xml"
expect_output "and so is a missing one" "$SCRATCH/missing.xml"
expect_output "the count" "2 flaky test(s) in 4 report(s)"

run_status python3 "$ROOT/scripts/lib/junit-flaky.py"
expect_status "no report at all is no failure either" 0

finish_tests "junit-flaky"
