#!/usr/bin/env bash
#
# scripts/ci-tree-greens.sh against a stub `gh` (RD-191-09 T02): an image is checked only by a
# successful run whose head commit has the same tree and whose `rust (<image>)` job succeeded,
# the union over several runs counts, and every failure to ask GitHub answers with every image.
# The per-run jobs after `--` (RA-TOOL-05) the same way, matched by name or matrix leg.
#
# Pure bash. check.sh runs it when scripts/ change, and under --full.
#
#   scripts/tests/ci-tree-greens.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export FAKE="$SCRATCH/fake"
mkdir -p "$FAKE" "$SCRATCH/bin"
cat > "$SCRATCH/bin/gh" <<'GH'
#!/usr/bin/env bash
echo "$*" >> "$FAKE/gh.calls"
[[ ! -e "$FAKE/down" ]] || exit 1
case "$1 $2" in
    "api repos/o/r/actions/workflows/ci.yml/runs?status=success&per_page=50") cat "$FAKE/runs" ;;
    "api repos/o/r/git/commits/"*) cat "$FAKE/tree-${2##*/}" ;;
    "api repos/o/r/actions/runs/"*) run="${2#repos/o/r/actions/runs/}"; cat "$FAKE/jobs-${run%%/*}" ;;
    *) echo "fake gh: unexpected $*" >&2; exit 1 ;;
esac
GH
chmod +x "$SCRATCH/bin/gh"
export PATH="$SCRATCH/bin:$PATH"
IMAGES=(ubuntu-24.04 windows-2025 macos-15)
greens() { run_status "$ROOT/scripts/ci-tree-greens.sh" o/r TREE "${IMAGES[@]}"; json="$(tail -n 1 <<< "$output")"; }

# Run 1: the same tree, Linux and Windows green. Run 2: the same tree, a dispatch for macOS alone.
# Run 3: another tree, everything green. Run 4: same commit as run 1 (the tree is asked once).
printf '1 aaa\n2 bbb\n3 ccc\n4 aaa\n' > "$FAKE/runs"
echo TREE > "$FAKE/tree-aaa"; echo TREE > "$FAKE/tree-bbb"; echo OTHER > "$FAKE/tree-ccc"
printf 'web\nrust (ubuntu-24.04)\nrust (windows-2025)\ncrash-matrix (ubuntu-24.04)\n' > "$FAKE/jobs-1"
printf 'web\nrust (macos-15)\n' > "$FAKE/jobs-2"
printf 'rust (ubuntu-24.04)\nrust (windows-2025)\nrust (macos-15)\n' > "$FAKE/jobs-3"
printf 'web\n' > "$FAKE/jobs-4"

greens
expect_status "answers" 0
expect "the union of two runs on the same tree covers every image" "[]" "$json"
expect_output "and says which were green" "macos-15: green for tree TREE"
expect "each commit's tree is asked once" "1" "$(grep -c 'git/commits/aaa' "$FAKE/gh.calls")"
expect_true "another tree's run is never asked for its jobs" '! grep -q "actions/runs/3/" "$FAKE/gh.calls"'

printf '1 aaa\n3 ccc\n' > "$FAKE/runs"
greens
expect "macOS left when only Linux and Windows were green on this tree" '["macos-15"]' "$json"

printf 'web\nrust (ubuntu-24.04)\n' > "$FAKE/jobs-1"
greens
expect "a job the run did not pass does not count" '["windows-2025","macos-15"]' "$json"

printf '3 ccc\n' > "$FAKE/runs"
greens
expect "an untested tree checks everything" '["ubuntu-24.04","windows-2025","macos-15"]' "$json"

: > "$FAKE/runs"
greens
expect "no successful run at all checks everything" '["ubuntu-24.04","windows-2025","macos-15"]' "$json"

touch "$FAKE/down"
greens
expect_status "GitHub not answering is no failure" 0
expect "and checks everything" '["ubuntu-24.04","windows-2025","macos-15"]' "$json"
expect_output "saying why" "::warning::ci-tree-greens: the runs could not be listed"
rm "$FAKE/down"

# The jobs that run once per run (RA-TOOL-05): named after `--`, a second JSON line.
JOBS=(scripts "docker" s3-live)
with_jobs() {
    run_status "$ROOT/scripts/ci-tree-greens.sh" o/r TREE "${IMAGES[@]}" -- "${JOBS[@]}"
    json="$(grep '^\[' <<< "$output" | head -n 1)"; jobs_json="$(grep '^\[' <<< "$output" | sed -n 2p)"
}
printf '1 aaa\n2 bbb\n3 ccc\n' > "$FAKE/runs"
printf 'web\nrust (ubuntu-24.04)\nrust (windows-2025)\nscripts\ndocker (amd64)\ndocker (arm64)\n' > "$FAKE/jobs-1"
printf 'web\nrust (macos-15)\n' > "$FAKE/jobs-2"
printf 's3-live\nrust (ubuntu-24.04)\n' > "$FAKE/jobs-3"
with_jobs
expect_status "answers with jobs" 0
expect "every image green on this tree" '[]' "$json"
expect "a job green on this tree, also as matrix legs, is not run again; another tree's is" \
    '["s3-live"]' "$jobs_json"
expect_output "and says which jobs were green" "docker: green for tree TREE"

printf 'web\nrust (ubuntu-24.04)\nscripts-extra\n' > "$FAKE/jobs-1"
with_jobs
expect "a job is matched by its whole name or its matrix legs, never a prefix" \
    '["scripts","docker","s3-live"]' "$jobs_json"

run_status "$ROOT/scripts/ci-tree-greens.sh" o/r TREE "${IMAGES[@]}"
expect "without -- there is no second line" "1" "$(grep -c '^\[' <<< "$output")"

touch "$FAKE/down"
with_jobs
expect "GitHub not answering runs every job" '["scripts","docker","s3-live"]' "$jobs_json"
rm "$FAKE/down"

run_status "$ROOT/scripts/ci-tree-greens.sh" o/r
expect_status "without images it is a usage error" 2
run_status "$ROOT/scripts/ci-tree-greens.sh" o/r TREE -- scripts
expect_status "jobs without images are a usage error" 2

finish_tests "ci-tree-greens"
