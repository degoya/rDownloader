#!/usr/bin/env bash
#
# The web/dist trap of a feature worktree (RD-1120-06, audit P10/A4; scripts/lib/web-dist.sh)
# against a scratch repository: a linked worktree with a web/dist of its own that builds in the
# main checkout's target is refused — by the library, by check.sh before its lock, warned about by
# `worktree.sh check` — while the link, the main checkout and a worktree with a target of its own
# pass; check.sh --preflight, which compiles nothing, is not refused. The packaging scripts and
# e2e.sh --build call the same guard.
#
# Pure git and bash, no cargo.
#
#   scripts/tests/web-dist-guard.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
# shellcheck source=../lib/web-dist.sh
source "$ROOT/scripts/lib/web-dist.sh"

export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
unset CARGO_TARGET_DIR RD_LANE_TARGET_DIR

MAIN="$SCRATCH/repo"
git init -q -b development "$MAIN"
mkdir -p "$MAIN/scripts/lib" "$MAIN/web/dist"
cp "$ROOT/scripts/check.sh" "$ROOT/scripts/worktree.sh" "$MAIN/scripts/"
cp "$ROOT/scripts/lib/"*.sh "$MAIN/scripts/lib/"
printf 'dist/\n' > "$MAIN/web/.gitignore"
echo '<html></html>' > "$MAIN/web/dist/index.html"
git -C "$MAIN" add -A
git -C "$MAIN" commit -qm base
WORKTREE="$SCRATCH/repo-feature"
git -C "$MAIN" worktree add -q -b feature "$WORKTREE" development 2> /dev/null

ln -s "$MAIN/web/dist" "$WORKTREE/web/dist"
expect_true "a worktree with the link: not in the trap" '! rd_web_dist_trap "$WORKTREE"'
expect_true "the main checkout with its own web/dist: not in the trap" '! rd_web_dist_trap "$MAIN"'

rm "$WORKTREE/web/dist"
mkdir -p "$WORKTREE/web/dist"
echo '<html>this branch</html>' > "$WORKTREE/web/dist/index.html"
expect_true "a worktree with a web/dist of its own in the shared target: in the trap" 'rd_web_dist_trap "$WORKTREE"'
run_status rd_web_dist_guard "$WORKTREE" "the test"
expect_status "the guard refuses it" 1
expect_output "naming the caller" "the test is refused"
expect_output "and how to restore the link" "ln -s '$MAIN/web/dist' '$WORKTREE/web/dist'"
expect_true "the same with a CARGO_TARGET_DIR every checkout shares" \
    'CARGO_TARGET_DIR="$SCRATCH/shared" rd_web_dist_trap "$WORKTREE"'

: > "$(rd_own_target_flag "$WORKTREE")"
expect_true "a worktree with a target of its own: not in the trap" '! rd_web_dist_trap "$WORKTREE"'
rm -f "$(rd_own_target_flag "$WORKTREE")"

# check.sh refuses before its lock, so this never reaches a build; a lock of its own that is
# never waited for keeps a broken guard away from the real one.
run_status env -u RD_LOCK_HELD RD_LOCK_FILE="$SCRATCH/lock" RD_LOCK_WAIT=0 "$WORKTREE/scripts/check.sh" --full
expect_status "check.sh --full in the trap: refused" 2
expect_output "by the guard" "scripts/check.sh is refused"
run_status env -u RD_LOCK_HELD RD_LOCK_FILE="$SCRATCH/lock" RD_LOCK_WAIT=0 RD_CHECK_LOGS="$SCRATCH/logs" \
    "$WORKTREE/scripts/check.sh" --preflight
expect_true "check.sh --preflight compiles nothing and is not refused" '! grep -q "is refused" <<< "$output"'

run_status "$MAIN/scripts/worktree.sh" check feature
expect_status "worktree.sh check only warns" 0
expect_output "naming the trap" "is refused: $WORKTREE/web/dist is a directory of its own"

for script in check.sh package-linux.sh package-windows.sh e2e.sh; do
    expect_true "$script calls the guard" 'grep -q "rd_web_dist_guard" "$ROOT/scripts/$script"'
done

finish_tests web-dist-guard
