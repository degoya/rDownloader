#!/usr/bin/env bash
#
# The evidence machinery of scripts/release-pipeline.sh (RD-140-22), for what it refuses: the
# gate before the tag passes only when every required step of *this* run has a passing,
# non-empty record, and run_step reads a step's exit status through the tee and counts a silent
# success as missing evidence.
#
# The pipeline is sourced with RELEASE_PIPELINE_LIB=1 from a scratch copy, so its evidence log
# lands in the scratch tree and not in this checkout's artifacts/.
#
#   scripts/tests/release-evidence.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
TREE="$SCRATCH/tree"
mkdir -p "$TREE/scripts/lib"
cp "$ROOT/scripts/release-pipeline.sh" "$TREE/scripts/"
cp "$ROOT/scripts/lib/"{lock,lanes,verified,jobs,public-ci}.sh "$TREE/scripts/lib/"
git init -q -b development "$TREE"
git -C "$TREE" commit -q --allow-empty -m base

# Each case runs in a subshell of its own: the pipeline's `exit` on a failed step must end the
# case, not this script.
pipeline() {
    # shellcheck disable=SC1091  # the scratch copy
    RELEASE_PIPELINE_LIB=1 source "$TREE/scripts/release-pipeline.sh" 9.9.9
}
LOG="$TREE/artifacts/release-evidence-9.9.9.log"
record_all() {
    local id
    for id in "${GATE_REQUIRES[@]}"; do
        printf '##RD-STEP id=%s nonce=%s version=9.9.9 exit=0 bytes=12 started=x ended=y\n' "$id" "$NONCE" >> "$LOG"
    done
}

run_status bash -c "$(declare -f pipeline record_all); LOG='$LOG'; TREE='$TREE'
    pipeline; record_all; step_evidence_gate"
expect_status "every required step green and non-empty: the gate passes" 0
expect_output "and says so" "have passing, non-empty evidence"

gate_with() {
    run_status bash -c "$(declare -f pipeline record_all); LOG='$LOG'; TREE='$TREE'
        pipeline; record_all; $1; step_evidence_gate"
}

gate_with "sed -i '/id=smoke /d' \"\$LOG\""
expect_status "a step without a record: refused" 1
expect_output "naming it" "smoke              NO EVIDENCE"

gate_with "printf '##RD-STEP id=test nonce=%s version=9.9.9 exit=101 bytes=5 started=x ended=y\n' \"\$NONCE\" >> \"\$LOG\""
expect_status "the last attempt at a step failed: refused" 1
expect_output "with its exit code" "exit=101"

gate_with "printf '##RD-STEP id=web nonce=%s version=9.9.9 exit=0 bytes=0 started=x ended=y\n' \"\$NONCE\" >> \"\$LOG\""
expect_status "a green step without output: refused" 1
expect_output "as missing evidence" "exit=0 but 0 bytes of output"

gate_with "sed -i \"/id=commit /s/nonce=\$NONCE/nonce=an-older-run/\" \"\$LOG\""
expect_status "a record from another run counts for nothing" 1
expect_output "for the step it belonged to" "commit             NO EVIDENCE"

gate_with "printf '##RD-STEP id=test nonce=%s version=9.9.9 exit=1 bytes=5 started=x ended=y\n' \"\$NONCE\" >> \"\$LOG\"; printf '##RD-STEP id=test nonce=%s version=9.9.9 exit=0 bytes=9 started=x ended=y\n' \"\$NONCE\" >> \"\$LOG\""
expect_status "a failure fixed by a later green attempt: passes" 0

gate_with "sed -i '/^##RD-RELEASE/d' \"\$LOG\""
expect_status "a log without this run's header: refused" 1

run_step_case() {
    run_status bash -c "$(declare -f pipeline); TREE='$TREE'; pipeline; $1"
}
run_step_case "run_step smoke bash -c 'echo some output; exit 7'"
expect_status "run_step: the step's own exit code, read through the tee" 7
expect_output "and the pipeline stops" "[smoke] failed with exit 7"
run_step_case "run_step smoke true"
expect_status "run_step: a silent success is missing evidence" 1
expect_output "said as such" "that is missing evidence, not a pass"
run_step_case "run_step smoke echo fine; grep -c '^##RD-STEP id=smoke .* exit=0 bytes=5 ' \"\$LOG\""
expect_status "run_step: a passing step records its exit and bytes" 0

finish_tests release-evidence
