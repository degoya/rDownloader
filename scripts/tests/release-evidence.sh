#!/usr/bin/env bash
#
# The evidence machinery of scripts/release-pipeline.sh (RD-140-22), for what it refuses: the
# gate before the tag passes only when every required step of *this* run has a passing,
# non-empty record, and run_step reads a step's exit status through the tee and counts a silent
# success as missing evidence. The `public-ci` step dispatches ci.yml and the release workflows
# that no recorded green covers, and a red one holds the tag (RD-1120-07).
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
cp "$ROOT/scripts/lib/"{lock,lanes,verified,jobs,public-ci,release-tag,release-evidence,release-steps-build,release-steps-publish,workspace-version,inert-paths}.sh "$TREE/scripts/lib/"
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
expect_status "public-ci with every platform but no release workflow green on record: needs a run" 1
expect_output "and names them" "installers.yml: to run"
for workflow in e2e.yml recovery.yml self-update.yml installers.yml; do
    printf '%s %s 2026-09-28T00:00:00+00:00\n' "$workflow" "$tree" >> "$TREE/.git/rd-verified-ci"
done
run_step_case "PATH='$SCRATCH/bin':\$PATH step_public_ci"
expect_status "public-ci with every platform and release workflow green on record: passes without gh" 0
expect_output "saying so" "the public CI is not run again"
rm -f "$TREE/.git/rd-verified-ci"

# The release workflows beside ci.yml (owner, 2026-10-06, RD-1120-07): dispatched on the
# candidate's branch, waited for with it, recorded on green; a red one holds the tag. The gh here
# answers `run list` from $FAKE/runs; the export commits into a scratch clone of a bare repository.
export FAKE="$SCRATCH/fake" RD_PUBLIC_DIR="$SCRATCH/public" RD_PUBLIC_REPO=owner/repo RD_PUBLIC_CI_POLL=0
mkdir -p "$FAKE" "$SCRATCH/gh-bin"
cat > "$SCRATCH/gh-bin/gh" <<'EOF'
#!/usr/bin/env bash
echo "$*" >> "$FAKE/gh.calls"
case "$1 $2" in
    "auth status"|"workflow run") exit 0 ;;
    "run list") cat "$FAKE/runs" ;;
    "run view") [[ "$*" == *"--json jobs"* ]] || grep "^$3 " "$FAKE/runs" ;;
    *) exit 1 ;;
esac
EOF
chmod +x "$SCRATCH/gh-bin/gh"
cat > "$TREE/scripts/export-public.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
branch="$(sed -n 's/.*--branch \([^ ]*\).*/\1/p' <<< "$*")"
git -C "$RD_PUBLIC_DIR" checkout -q -B "$branch"
git -C "$RD_PUBLIC_DIR" commit -q --allow-empty -m export
git -C "$RD_PUBLIC_DIR" push -q --force origin "$branch"
git -C "$RD_PUBLIC_DIR" checkout -q main
EOF
chmod +x "$TREE/scripts/export-public.sh"
git init -q --bare -b main "$SCRATCH/public.git"
git clone -q "$SCRATCH/public.git" "$RD_PUBLIC_DIR" 2> /dev/null
git -C "$RD_PUBLIC_DIR" commit -q --allow-empty -m main
git -C "$RD_PUBLIC_DIR" push -q origin main
public_ci_case() {
    rm -f "$FAKE/gh.calls"
    run_step_case "PATH='$SCRATCH/gh-bin':\$PATH step_public_ci"
}

printf '%s %s 2026-09-28T00:00:00+00:00\n' ubuntu-24.04 "$tree" windows-2025 "$tree" > "$TREE/.git/rd-verified-ci"
printf '%s\n' "1 completed success https://example.invalid/runs/1 CI" \
    "2 completed success https://example.invalid/runs/2 E2E" > "$FAKE/runs"
public_ci_case
expect_status "macOS and the release workflows, green: passes" 0
expect_true "ci.yml for macOS alone, without the once-per-run jobs Linux and Windows passed" \
    'grep -qxF "workflow run ci.yml --repo owner/repo --ref ci/9.9.9 -f platforms=[\"macos-15\"] -f jobs=[]" "$FAKE/gh.calls"'
expect "every release workflow dispatched on the candidate's branch" \
    "e2e.yml installers.yml recovery.yml self-update.yml" \
    "$(sed -n 's/^workflow run \([a-z0-9-]*\.yml\) --repo owner\/repo --ref ci\/9.9.9$/\1/p' "$FAKE/gh.calls" | sort | paste -sd' ')"
expect "macOS and the four workflows recorded" \
    "e2e.yml installers.yml macos-15 recovery.yml self-update.yml" \
    "$(awk -v tree="$tree" 'NR > 2 && $2 == tree { print $1 }' "$TREE/.git/rd-verified-ci" | sort | paste -sd' ')"

public_ci_case
expect_status "the same content again: passes" 0
expect_true "without asking GitHub" '[[ ! -f "$FAKE/gh.calls" ]]'

sed -i '/^e2e.yml /d' "$TREE/.git/rd-verified-ci"
echo "3 completed failure https://example.invalid/runs/3 E2E" > "$FAKE/runs"
public_ci_case
expect_status "a red release workflow holds the tag" 1
expect_output "saying so" "the tag is not made while the public CI is not green"
expect_true "ci.yml is not dispatched for platforms green on record" '! grep -q "^workflow run ci.yml" "$FAKE/gh.calls"'
expect_true "only the workflow not yet green is" \
    'grep -qx "workflow run e2e.yml --repo owner/repo --ref ci/9.9.9" "$FAKE/gh.calls" && [[ $(grep -c "^workflow run" "$FAKE/gh.calls") -eq 1 ]]'
expect_true "and the red one is not recorded" '! grep -q "^e2e.yml " "$TREE/.git/rd-verified-ci"'
rm -f "$TREE/.git/rd-verified-ci" "$TREE/scripts/export-public.sh"
unset FAKE RD_PUBLIC_DIR RD_PUBLIC_REPO RD_PUBLIC_CI_POLL

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
# The release's notes for users (RD-1150-02): a section still marked as a draft stops the gate.
cp "$ROOT/scripts/release-notes.sh" "$TREE/scripts/"
printf '# Release notes\n\n## 9.9.9\n\n<!-- draft -->\n\n- Something new.\n' > "$TREE/RELEASE-NOTES.md"
run_step_case "RD_WIKI_SRC='$SCRATCH/no-wiki' step_docs_gate"
expect_output "docs-gate: the previous release is the highest tag under this one" "previous version: 9.9.8"
expect_output "docs-gate: a changelog unchanged since that release is refused" \
    "CHANGELOG.md is unchanged since v9.9.8"
expect_output "docs-gate: the release's notes for users must be finished" \
    "the section 9.9.9 is still a draft"
rm "$TREE/RELEASE-NOTES.md" "$TREE/scripts/release-notes.sh"

# A pre-release, X.Y.Z-beta.N and no other form (owner, 2026-09-30): not merged into main, so
# neither run nor required by the gate; pushed without main; published by the export alone.
run_status bash "$TREE/scripts/release-pipeline.sh" 9.9.9-rc.1 --plan
expect_status "a pre-release other than a beta: refused" 2
expect_output "naming the forms" "X.Y.Z-beta.N for a pre-release"
run_status bash "$TREE/scripts/release-pipeline.sh" 9.9.9-beta.1 --plan
expect_status "a beta is a release version" 0
expect_output "--plan says it is a pre-release" "release 9.9.9-beta.1 (pre-release)"
expect_output "--plan skips merge-main for it" "  - merge-main (skipped: a pre-release is not merged into main)"
expect_output "--plan publishes the export only" "  - publish-public (the public export only: no wiki, no website)"

pipeline_beta() {
    # shellcheck disable=SC1091  # the scratch copy
    RELEASE_PIPELINE_LIB=1 source "$TREE/scripts/release-pipeline.sh" 9.9.9-beta.1
}
beta_case() {
    run_status bash -c "$(declare -f pipeline_beta record_all); TREE='$TREE'
        LOG='$TREE/artifacts/release-evidence-9.9.9-beta.1.log'; pipeline_beta; $1"
}
beta_case "record_all; step_evidence_gate"
expect_status "the gate of a beta passes without a merge-main record" 0
expect_true "and never asks for one" '! grep -qF "merge-main" <<< "$output"'
gate_with "sed -i '/id=merge-main /d' \"\$LOG\""
expect_status "the gate of a stable release still requires merge-main" 1
expect_output "naming it" "merge-main         NO EVIDENCE"

# The three publishing scripts as stubs that note their call.
for stub in export-public export-wiki update-website; do
    printf '#!/usr/bin/env bash\necho "%s $*" >> "%s/published"\n' "$stub" "$SCRATCH" > "$TREE/scripts/$stub.sh"
    chmod +x "$TREE/scripts/$stub.sh"
done
beta_case "step_publish_public"
expect_status "publish-public of a beta passes" 0
expect "and runs the public export alone" "export-public 9.9.9-beta.1" "$(cat "$SCRATCH/published")"
rm -f "$SCRATCH/published"
run_step_case "step_publish_public"
expect "a stable release publishes all three" \
    "export-public 9.9.9 export-wiki 9.9.9 update-website 9.9.9" "$(tr '\n' ' ' < "$SCRATCH/published" | sed 's/ $//')"

git init -q --bare "$SCRATCH/origin.git"
git -C "$TREE" remote add origin "$SCRATCH/origin.git"
git -C "$TREE" tag v9.9.9-beta.1
beta_case "step_push"
expect_status "push of a beta passes" 0
expect "and publishes the release branch and the tag, not main" \
    "refs/heads/development refs/tags/v9.9.9-beta.1" \
    "$(git -C "$SCRATCH/origin.git" for-each-ref --format='%(refname)' | sort | tr '\n' ' ' | sed 's/ $//')"

# step_preflight runs check.sh --preflight, and red stops the release before it builds anything
# (RD-1120-06, audit A2). The tools it asks for and pgrep are stand-ins; check.sh is one that
# notes its call and fails while $SCRATCH/preflight-red exists.
mkdir -p "$SCRATCH/tools"
for tool in cargo cargo-nextest node pnpm python3 zip; do
    printf '#!/usr/bin/env bash\necho "%s 0.0.0"\n' "$tool" > "$SCRATCH/tools/$tool"
    chmod +x "$SCRATCH/tools/$tool"
done
printf '#!/usr/bin/env bash\necho 0\nexit 1\n' > "$SCRATCH/tools/pgrep"
chmod +x "$SCRATCH/tools/pgrep"
printf '#!/usr/bin/env bash\necho "check.sh $* layout-skip=${RD_SKIP_JOB_LAYOUT:-}" >> "%s/check.calls"\n[[ ! -f "%s/preflight-red" ]] || { echo "!! 1 stage(s) failed"; exit 1; }\necho "==> all requested checks passed"\n' \
    "$SCRATCH" "$SCRATCH" > "$TREE/scripts/check.sh"
chmod +x "$TREE/scripts/check.sh"
printf 'scripts/\nartifacts/\n' >> "$TREE/.git/info/exclude"
git -C "$TREE" checkout -q development
export CARGO_TARGET_DIR="$SCRATCH/target"
# shellcheck source=../lib/verified.sh
(source "$ROOT/scripts/lib/verified.sh" && rd_record_verified "$TREE" "$(git -C "$TREE" rev-parse HEAD)")
preflight_case() {
    run_status bash -c "$(declare -f pipeline); TREE='$TREE'; pipeline; PATH='$SCRATCH/tools':\$PATH step_preflight"
}
preflight_case
expect_status "step_preflight with a green preflight: passes" 0
expect "and ran check.sh --preflight, the job layout left to the archive-jobs step" \
    "check.sh --preflight layout-skip=1" "$(cat "$SCRATCH/check.calls")"
expect_output "saying so" "preflight ok at"
touch "$SCRATCH/preflight-red"
preflight_case
expect_status "a red preflight stops the release" 1
expect_output "saying why" "the preflight is red; the release does not start"
rm -f "$SCRATCH/preflight-red"

finish_tests release-evidence
