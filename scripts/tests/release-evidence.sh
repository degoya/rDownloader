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
cp "$ROOT/scripts/lib/"{lock,lanes,verified,jobs,public-ci,release-tag}.sh "$TREE/scripts/lib/"
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

gate_with "sed -i '/id=compat /d' \"\$LOG\""
expect_status "the compatibility gate is required evidence (RD-170-08)" 1
expect_output "naming it" "compat             NO EVIDENCE"

run_status bash "$TREE/scripts/release-pipeline.sh" 9.9.9 --plan
expect_status "--plan prints the steps" 0
expect "--plan: compat right after preflight, before the long steps" "  - compat" \
    "$(grep -A1 -x -- '  - preflight' <<< "$output" | tail -1)"

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

# The public CI with every platform green on record for this content: no run (RD-160-06). The gh
# here refuses everything, so a step that wanted GitHub fails.
mkdir -p "$SCRATCH/bin"
printf '#!/usr/bin/env bash\necho "gh must not be called" >&2\nexit 1\n' > "$SCRATCH/bin/gh"
chmod +x "$SCRATCH/bin/gh"
tree="$(git -C "$TREE" rev-parse 'HEAD^{tree}')"
printf '%s %s 2026-09-28T00:00:00+00:00\n' ubuntu-24.04 "$tree" windows-2025 "$tree" > "$TREE/.git/rd-verified-ci"
run_step_case "PATH='$SCRATCH/bin':\$PATH step_public_ci"
expect_status "public-ci with macOS not yet green: needs a run" 1
expect_output "and names it" "macos-15: to run"
printf '%s %s 2026-09-28T00:00:00+00:00\n' macos-15 "$tree" >> "$TREE/.git/rd-verified-ci"
run_step_case "PATH='$SCRATCH/bin':\$PATH step_public_ci"
expect_status "public-ci with every platform green on record: passes without gh" 0
expect_output "saying so" "the public CI is not run again"
rm -f "$TREE/.git/rd-verified-ci"

# The checkout ends on the release branch, not on main (RD-160-06).
git -C "$TREE" branch -q main
git -C "$TREE" checkout -q main
run_step_case "return_to_release_branch; git rev-parse --abbrev-ref HEAD"
expect_status "from main: back on the release branch" 0
expect_output "saying so" "the checkout is back on development"
expect "the checkout is on development" "development" "$(git -C "$TREE" rev-parse --abbrev-ref HEAD)"
run_step_case "return_to_release_branch"
expect_status "on the release branch already: nothing to do" 0
expect "and nothing said" "" "$output"

# docs-gate compares the changelog with the last shipped release, the highest tag under the one
# being cut — not with what `git describe` answers on development, where the release tags on
# main's merge commits are never reachable and an older tag is.
printf '## [9.8.0]\n' > "$TREE/CHANGELOG.md"
git -C "$TREE" add CHANGELOG.md && git -C "$TREE" commit -qm "release 9.8.0" && git -C "$TREE" tag v9.8.0
printf '## [9.9.8]\n\n## [9.8.0]\n' > "$TREE/CHANGELOG.md"
git -C "$TREE" commit -qam "work towards 9.9.8"
git -C "$TREE" checkout -q main
git -C "$TREE" merge -q --no-ff -m "release 9.9.8" development && git -C "$TREE" tag v9.9.8
# A tag above the release being cut is no release it comes after.
git -C "$TREE" tag v10.0.0
git -C "$TREE" checkout -q development
expect "the scenario: git describe on development answers the older tag" "v9.8.0" \
    "$(git -C "$TREE" describe --tags --abbrev=0)"
run_step_case "RD_WIKI_SRC='$SCRATCH/no-wiki' step_docs_gate"
expect_output "docs-gate: the previous release is the highest tag under this one" "previous version: 9.9.8"
expect_output "docs-gate: a changelog unchanged since that release is refused" \
    "CHANGELOG.md is unchanged since v9.9.8"

finish_tests release-evidence
