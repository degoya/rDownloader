#!/usr/bin/env bash
#
# scripts/job-evidence.sh (RD-1270-14) on a wave of two jobs: the status, the evidence line from
# check.log, .git/rd-verified-ci and public-ci.log, the open boxes marked and copied into
# RD-1120-24 — compared with expected files —, a second run and one after archive-jobs.sh that
# write nothing, and every refusal leaving the files as they were.
#
# The fixtures are scripts/tests/fixtures/job-evidence/: `logs/` the integration's log directory
# (excerpts in the shape of integration/1.26-w1), `jobs/` the job files before the run, `expected/`
# after it. `@COMMIT@`, `@TREE@` and `@GENERATORS@` are filled in from the scratch repository
# built here. The job files end in `.in` so that a change to them is not documentation
# (rd_inert_path) and runs this test.
#
#   scripts/tests/job-evidence.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FIXTURES="$ROOT/scripts/tests/fixtures/job-evidence"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
# The local greens job-evidence.sh reads for Linux (RD-1280-14) live here, never in a real target.
export CARGO_TARGET_DIR="$SCRATCH/target"
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
TREE="$SCRATCH/tree"
JOBS="$TREE/docs/roadmap/jobs"
mkdir -p "$TREE/scripts/lib" "$JOBS" "$TREE/web" "$TREE/crates/rd-api/licenses"
cp "$ROOT/scripts/job-evidence.sh" "$TREE/scripts/"
cp "$ROOT/scripts/lib/"{verified,verified-ci,lanes,inert-paths,workspace-version}.sh "$TREE/scripts/lib/"
for file in "$FIXTURES"/jobs/*.in; do
    cp "$file" "$JOBS/$(basename "${file%.in}")"
done
git init -q -b development "$TREE"
git -C "$TREE" add -A
git -C "$TREE" commit -q -m base
# An older wave's generators, then this wave's: the line names this one.
echo '[]' > "$TREE/crates/rd-api/licenses/third-party.json"
git -C "$TREE" add -A
git -C "$TREE" commit -q -m "chore(generated): regenerate after merging feat/1298-01-old"
echo '{}' > "$TREE/web/openapi.json"
echo 'x' > "$TREE/crates/rd-api/mcp-coverage.md"
echo 'y' > "$TREE/web/components.d.ts"
git -C "$TREE" add -A
git -C "$TREE" commit -q -m "chore(generated): regenerate after merging feat/1299-01-alpha fix/1299-02-beta"
git -C "$TREE" branch integration/1.99-w1
COMMIT="$(git -C "$TREE" rev-parse --short=9 HEAD)"
TREE_ID="$(git -C "$TREE" rev-parse 'HEAD^{tree}')"
OTHER_TREE="$(git -C "$TREE" rev-parse 'HEAD~2^{tree}')"

LOGS="$SCRATCH/integration-1.99-w1"
fill() {
    sed -e "s/@COMMIT@/$COMMIT/g" -e "s/@TREE@/${TREE_ID:0:12}/g" -e "s/@GENERATORS@/$COMMIT/g" "$1"
}
reset_logs() {
    rm -rf "$LOGS"
    mkdir -p "$LOGS"
    for file in status check.log public-ci.log; do fill "$FIXTURES/logs/$file" > "$LOGS/$file"; done
    touch -d '2026-10-12 12:00' "$LOGS/status"
    printf '%s %s 2026-10-12T12:00:00+02:00\n' ubuntu-24.04 "$TREE_ID" windows-2025 "$TREE_ID" \
        e2e.yml "$TREE_ID" macos-15 "$OTHER_TREE" > "$TREE/.git/rd-verified-ci"
}
digest() {
    (cd "$JOBS" && find . -type f -print0 | sort -z | xargs -0 sha256sum)
}
evidence() {
    bash "$TREE/scripts/job-evidence.sh" "$LOGS" "$@"
}

# --- the run ---------------------------------------------------------------------------------
reset_logs
# shellcheck disable=SC2016  # the backticks are Markdown
NOTE='`alpha_tests.rs` ist darin'
run_status evidence RD-1299-01="$NOTE" 1299-02
expect_status "a green wave: the jobs are closed" 0
expect_output "naming the run" "integration/1.99-w1 @ $COMMIT (tree ${TREE_ID:0:12}): --full, GitHub ubuntu-24.04 und windows-2025 (run 123456)"
expect_output "and what it wrote" "written:   docs/roadmap/jobs/1120-24-die-offenen-live-abnahmen.md"
for file in 1299-01-alpha.md 1299-02-beta.md 1120-24-die-offenen-live-abnahmen.md; do
    if diff_out="$(diff <(fill "$FIXTURES/expected/$file.in") "$JOBS/$file")"; then
        ok "$file as expected"
    else
        fail "$file as expected" "$diff_out"
    fi
done

before="$(digest)"
run_status evidence RD-1299-01="$NOTE" 1299-02
expect_status "a second run" 0
expect "writes nothing" "$before" "$(digest)"
expect_output "and says so" "unchanged: docs/roadmap/jobs/1299-01-alpha.md"

mkdir -p "$JOBS/archive"
mv "$JOBS/1299-01-alpha.md" "$JOBS/1299-02-beta.md" "$JOBS/archive/"
before="$(digest)"
run_status evidence 1299-01 RD-1299-02
expect_status "a run after archive-jobs.sh moved the jobs" 0
expect "finds them in archive/ and writes nothing" "$before" "$(digest)"

# --- refusals: nothing written ---------------------------------------------------------------
git -C "$TREE" checkout -q -- docs
git -C "$TREE" clean -fdq docs
refused() {
    local name="$1" needle="$2" before
    shift 2
    before="$(digest)"
    run_status evidence "$@"
    expect_status "$name: refused" 1
    expect_output "$name: with the reason" "$needle"
    expect "$name: nothing written" "$before" "$(digest)"
    reset_logs
}

sed -i 's/full=0/full=1/' "$LOGS/status"
refused "a red --full" "full=1" 1299-01
sed -i 's/windows=0/windows=101/' "$LOGS/status"
refused "a red Windows lint" "windows=101 is red" 1299-01
sed -i 's/public-ci=0/public-ci=1/' "$LOGS/status"
refused "a red GitHub run" "public-ci=1 is red" 1299-01
sed -i '/^==> all requested checks passed$/d' "$LOGS/check.log"
refused "check.log without its closing line" "lacks its closing line" 1299-01
sed -i '/^windows-2025 /d' "$TREE/.git/rd-verified-ci"
refused "no Windows green on record" "no GitHub green on Linux and Windows recorded" 1299-01
printf '%s %s 2026-10-12T12:00:00+02:00\n' ubuntu-24.04 "$OTHER_TREE" windows-2025 "$OTHER_TREE" > "$TREE/.git/rd-verified-ci"
refused "greens for another tree only" "no GitHub green on Linux and Windows recorded" 1299-01
sed -i '/^ubuntu-24.04 /d' "$TREE/.git/rd-verified-ci"
refused "no Linux green on GitHub and none local" "no GitHub green on Linux and Windows recorded" 1299-01
sed -i "s/recorded green at $COMMIT/recorded green at 0123456789ab/" "$LOGS/check.log"
refused "a commit this repository does not have" "is not in" 1299-01
refused "an unknown job" "RD-1299-09: 0 job files match" 1299-01 1299-09
refused "RD-1120-24 itself" "collects the open boxes" 1120-24
refused "no job id" "not a job id: alpha" alpha

run_status bash "$TREE/scripts/job-evidence.sh" "$LOGS"
expect_status "a log directory alone: the usage" 2

# --- without the Windows lint, without a public-ci.log ---------------------------------------
sed -i 's/windows=0/windows=skipped/; s/public-ci=0/public-ci=skipped/' "$LOGS/status"
rm "$LOGS/public-ci.log"
run_status evidence 1299-02
expect_status "--windows and public-ci.sh skipped, the greens on record" 0
line="$(grep '^— Nachweis der Welle' "$JOBS/1299-02-beta.md")"
if [[ "$line" != *"--windows"* ]]; then ok "names no Windows lint"; else fail "names no Windows lint" "$line"; fi
if [[ "$line" == *", GitHub CI grün auf ubuntu-24.04 und windows-2025 (dazu e2e.yml);"* ]]; then
    ok "names the GitHub greens without a run id"
else
    fail "names the GitHub greens without a run id" "$line"
fi

# --- Linux by the local --full green of the Rust half (RD-1280-14) ----------------------------
reset_logs
for file in "$FIXTURES"/jobs/*.in; do cp "$file" "$JOBS/$(basename "${file%.in}")"; done
sed -i '/^ubuntu-24.04 /d' "$TREE/.git/rd-verified-ci"
(source "$TREE/scripts/lib/verified.sh" && rd_record_full "$TREE" rust "$TREE_ID")
run_status evidence 1299-02
expect_status "Windows on GitHub, Linux by the local --full green" 0
expect_output "naming it" "--full, GitHub windows-2025 (run 123456), Linux by the local --full"
line="$(grep '^— Nachweis der Welle' "$JOBS/1299-02-beta.md" | tail -1)"
if [[ "$line" == *", GitHub CI 123456 grün auf windows-2025, Linux durch das lokale \`--full\` (dazu e2e.yml);"* ]]; then
    ok "the evidence line says so"
else
    fail "the evidence line says so" "$line"
fi

finish_tests "job-evidence"
