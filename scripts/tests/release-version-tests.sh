#!/usr/bin/env bash
#
# The release chain's test step after a reuse of the green before the version bump (RD-1300-06):
# the version-dependent tests of scripts/lib/version-tests.list still run, with RD_RELEASE_VERSION,
# one nextest run per line; the log names the reuse and the count; a red one makes the step red and
# carries nothing to the bumped tree. step_test is sourced from scripts/lib/release-steps-build.sh
# into a scratch tree whose verified.sh functions, check.sh and cargo are stand-ins: cargo answers
# each nextest run green, red for a filter named in $FAKE/red, or with no test for one in $FAKE/empty.
#
#   scripts/tests/release-version-tests.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
TREE="$SCRATCH/tree"
export FAKE="$SCRATCH/fake"
mkdir -p "$TREE/scripts/lib" "$FAKE" "$SCRATCH/bin"
cp "$ROOT/scripts/lib/release-steps-build.sh" "$ROOT/scripts/lib/version-tests.sh" \
    "$ROOT/scripts/lib/version-tests.list" "$TREE/scripts/lib/"
printf '#!/usr/bin/env bash\necho "check.sh $*" >> "$FAKE/check.calls"\n' > "$TREE/scripts/check.sh"
chmod +x "$TREE/scripts/check.sh"
git init -q "$TREE"
git -C "$TREE" add -A
git -C "$TREE" commit -qm base
cat > "$SCRATCH/bin/cargo" << 'EOF'
#!/usr/bin/env bash
filter=""
while [[ $# -gt 0 ]]; do [[ "$1" == -E ]] && filter="$2"; shift; done
echo "RD_RELEASE_VERSION=${RD_RELEASE_VERSION:-} $filter" >> "$FAKE/cargo.calls"
if [[ -f "$FAKE/empty" ]] && grep -qxF -- "$filter" "$FAKE/empty"; then
    echo "error: no tests to run"; exit 4
fi
if [[ -f "$FAKE/red" ]] && grep -qxF -- "$filter" "$FAKE/red"; then
    echo "        FAIL [   0.010s] a test reading the version"
    echo "     Summary [   0.020s] 2 tests run: 1 passed, 1 failed, 40 skipped"
    exit 100
fi
echo "     Summary [   0.020s] 2 tests run: 2 passed, 40 skipped"
EOF
chmod +x "$SCRATCH/bin/cargo"
entries="$(grep -cv '^#' "$TREE/scripts/lib/version-tests.list")"

# The test step in a shell of its own; $FAKE/prebump stands for a --full green of the tree before
# the bump, $FAKE/recorded for the green carried to the bumped tree.
step() {
    rm -f "$FAKE"/{cargo,check}.calls "$FAKE/recorded"
    run_status env PATH="$SCRATCH/bin:$PATH" bash -c "
        cd '$TREE'; ROOT='$TREE'; VERSION=9.9.9; JOBS=2
        rd_prebump_green() { [[ -f '$FAKE/prebump' ]] && echo pre-bump-tree; true; }
        rd_worktree_tree() { echo bumped-tree; }
        rd_full_marker() { echo marker; }
        rd_record_full() { echo \"\$2 \$3\" > '$FAKE/recorded'; }
        source '$TREE/scripts/lib/release-steps-build.sh'
        step_test"
}

touch "$FAKE/prebump"
step
expect_status "a reuse with every version-dependent test green: the step is green" 0
expect_output "the log names the reuse" "the full Rust run is not repeated"
expect_output "and the count of tests run" "version-dependent tests: $((entries * 2)) run in $entries runs with 9.9.9, all passed"
expect "one nextest run per line of the list" "$entries" "$(wc -l < "$FAKE/cargo.calls")"
expect "each with the version being released" "$entries" "$(grep -c '^RD_RELEASE_VERSION=9.9.9 ' "$FAKE/cargo.calls")"
expect_true "the release's newest baseline among them" \
    'grep -qxF "RD_RELEASE_VERSION=9.9.9 test(/^migration_forward::baselines::/)" "$FAKE/cargo.calls"'
expect "the green carried to the bumped tree" "rust bumped-tree" "$(cat "$FAKE/recorded")"
expect "no full run" "absent" "$([[ -e "$FAKE/check.calls" ]] && echo present || echo absent)"

echo 'test(/^migration_forward::baselines::/)' > "$FAKE/red"
step
expect_status "a reuse with a red version-dependent test: the step is red" 1
expect_output "naming the file" "crates/rd-db/tests/database/migration_forward/baselines.rs"
expect_output "with nextest's own output" "FAIL [   0.010s] a test reading the version"
expect "the other runs still ran" "$entries" "$(wc -l < "$FAKE/cargo.calls")"
expect "nothing carried to the bumped tree" "absent" "$([[ -e "$FAKE/recorded" ]] && echo present || echo absent)"
expect "and no full run in its place" "absent" "$([[ -e "$FAKE/check.calls" ]] && echo present || echo absent)"
rm -f "$FAKE/red"

echo 'test(/^tray_menu::tests::/)' > "$FAKE/empty"
step
expect_status "a line whose filter runs no test: red" 1
expect_output "naming it" "crates/rd-capture/src/tray_menu.rs"
rm -f "$FAKE/empty"

rm -f "$FAKE/prebump"
step
expect_status "no green before the bump: the full run" 0
expect "check.sh --rust --full, as before" "check.sh --rust --full" "$(cat "$FAKE/check.calls")"
expect "and no separate version-dependent run" "absent" "$([[ -e "$FAKE/cargo.calls" ]] && echo present || echo absent)"

finish_tests release-version-tests
