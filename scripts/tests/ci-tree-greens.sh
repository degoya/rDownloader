#!/usr/bin/env bash
#
# scripts/ci-tree-greens.sh against a stub `gh` (RD-191-09 T02): an image is checked only by a
# successful run whose head commit has the same tree and whose `rust (<image>)` job succeeded,
# the union over several runs counts, and every failure to ask GitHub answers with every image.
# The per-run jobs after `--` (RA-TOOL-05) the same way, matched by name or matrix leg. Another
# workflow's runs and matrix job by CI_TREE_GREENS_WORKFLOW and CI_TREE_GREENS_LEG, and a job
# without images, as e2e.yml's and recovery.yml's gates ask, and with CI_TREE_GREENS_UP_TO_VERSIONS
# a run whose commit differs only by a version bump and documentation (RD-1270-11).
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
    "api repos/o/r/actions/workflows/"*"/runs?status=success&per_page=50") cat "$FAKE/runs" ;;
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

expect_true "ci.yml's runs are the ones asked by default" \
    'grep -q "^api repos/o/r/actions/workflows/ci.yml/runs" "$FAKE/gh.calls"'

# Another workflow and its matrix job (RD-1270-11): e2e.yml's legs are `smoke (<image>)`.
: > "$FAKE/gh.calls"
printf '1 aaa\n' > "$FAKE/runs"
printf 'web\nsmoke (ubuntu-24.04)\nrust (windows-2025)\n' > "$FAKE/jobs-1"
CI_TREE_GREENS_WORKFLOW=e2e.yml CI_TREE_GREENS_LEG=smoke greens
expect "a leg of the named matrix job counts, another job's never" '["windows-2025","macos-15"]' "$json"
expect_true "the named workflow's runs are asked" \
    'grep -q "^api repos/o/r/actions/workflows/e2e.yml/runs" "$FAKE/gh.calls"'
expect "and no other workflow's" "0" "$(grep -c 'workflows/ci.yml' "$FAKE/gh.calls" || true)"

# A workflow without a matrix names no image, only its job (recovery.yml).
printf 'Kill and restart (axis B)\n' > "$FAKE/jobs-1"
run_status env CI_TREE_GREENS_WORKFLOW=recovery.yml "$ROOT/scripts/ci-tree-greens.sh" o/r TREE -- "Kill and restart (axis B)"
expect_status "jobs without images answer" 0
expect "with no image to check and the job green" "[] []" "$(grep '^\[' <<< "$output" | paste -sd' ')"
printf 'web\n' > "$FAKE/jobs-1"
run_status env CI_TREE_GREENS_WORKFLOW=recovery.yml "$ROOT/scripts/ci-tree-greens.sh" o/r TREE -- "Kill and restart (axis B)"
expect "and the job still to run when no run of the tree passed it" '[] ["Kill and restart (axis B)"]' \
    "$(grep '^\[' <<< "$output" | paste -sd' ')"
touch "$FAKE/down"
run_status "$ROOT/scripts/ci-tree-greens.sh" o/r TREE -- "Kill and restart (axis B)"
expect "GitHub not answering runs the job" '[] ["Kill and restart (axis B)"]' "$(grep '^\[' <<< "$output" | paste -sd' ')"
rm "$FAKE/down"

# Up to documentation and a version bump (RD-1270-11, CI_TREE_GREENS_UP_TO_VERSIONS): a run whose
# commit holds this tree up to the bump and documentation counts, one with anything more does not.
# The runs' commits live in an `origin` the checkout fetches them from.
ORIGIN="$SCRATCH/origin"
CHECKOUT="$SCRATCH/checkout"
git init -q "$ORIGIN"
commit_in() {
    git -C "$1" add -A
    git -C "$1" -c user.name=test -c user.email=test@example.invalid commit -q -m "$2"
    git -C "$1" rev-parse HEAD
}
manifest() { printf '[workspace.package]\nversion = "%s"\n' "$1" > "$2/Cargo.toml"; }
manifest 1.0.0 "$ORIGIN"
printf 'fn main() {}\n' > "$ORIGIN/main.rs"
echo "# Changes" > "$ORIGIN/CHANGELOG.md"
candidate="$(commit_in "$ORIGIN" candidate)"
git clone -q --depth=1 "file://$ORIGIN" "$CHECKOUT"
printf 'fn main() { }\n' > "$ORIGIN/main.rs"
edited="$(commit_in "$ORIGIN" "a source edit")"
git -C "$ORIGIN" checkout -q "$candidate"
printf '[workspace.package]\nversion = "1.0.0"\nedition = "2024"\n' > "$ORIGIN/Cargo.toml"
manifest_edit="$(commit_in "$ORIGIN" "more than the version")"
git -C "$ORIGIN" branch -q manifest-edit
# The tree pushed to main: the candidate with its version bumped and its notes written.
manifest 1.1.0 "$CHECKOUT"
printf '# Changes\n\n## 1.1.0\n' > "$CHECKOUT/CHANGELOG.md"
commit_in "$CHECKOUT" release > /dev/null
released="$(git -C "$CHECKOUT" rev-parse 'HEAD^{tree}')"
for sha in "$candidate" "$edited" "$manifest_edit"; do
    git -C "$ORIGIN" rev-parse "$sha^{tree}" > "$FAKE/tree-$sha"
done
printf 'smoke (ubuntu-24.04)\nsmoke (windows-2025)\n' > "$FAKE/jobs-1"
printf 'smoke (macos-15)\n' > "$FAKE/jobs-2"
printf 'smoke (macos-15)\n' > "$FAKE/jobs-3"
loose() {
    run_status env CI_TREE_GREENS_WORKFLOW=e2e.yml CI_TREE_GREENS_LEG=smoke "$@" \
        bash -c 'cd "$1" && shift && "$@"' _ "$CHECKOUT" "$ROOT/scripts/ci-tree-greens.sh" o/r "$released" "${IMAGES[@]}"
    json="$(grep '^\[' <<< "$output" | head -n 1)"
}
printf '1 %s\n2 %s\n3 %s\n' "$candidate" "$edited" "$manifest_edit" > "$FAKE/runs"
loose
expect "without CI_TREE_GREENS_UP_TO_VERSIONS only the exact tree counts" '["ubuntu-24.04","windows-2025","macos-15"]' "$json"
loose CI_TREE_GREENS_UP_TO_VERSIONS=1
expect_status "up to versions: answers" 0
expect "a candidate that differs by the bump and documentation counts; a source edit and a manifest edit do not" \
    '["macos-15"]' "$json"
expect_output "and says which commit counted" "run commit ${candidate:0:12}: tree ${released:0:12} up to documentation and versions"
printf '2 %s\n1 %s\n' "$edited" "$candidate" > "$FAKE/runs"
loose CI_TREE_GREENS_UP_TO_VERSIONS=1 CI_TREE_GREENS_FETCHES=1
expect "at most CI_TREE_GREENS_FETCHES commits are fetched" '["ubuntu-24.04","windows-2025","macos-15"]' "$json"
printf '1 %s\n' "0123456789abcdef0123456789abcdef01234567" > "$FAKE/runs"
echo OTHER > "$FAKE/tree-0123456789abcdef0123456789abcdef01234567"
loose CI_TREE_GREENS_UP_TO_VERSIONS=1
expect_status "a commit that cannot be fetched is no failure" 0
expect "and does not count" '["ubuntu-24.04","windows-2025","macos-15"]' "$json"

run_status "$ROOT/scripts/ci-tree-greens.sh" o/r
expect_status "without images it is a usage error" 2
run_status "$ROOT/scripts/ci-tree-greens.sh" o/r TREE --
expect_status "neither images nor jobs are a usage error" 2

finish_tests "ci-tree-greens"
