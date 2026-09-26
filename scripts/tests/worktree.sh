#!/usr/bin/env bash
#
# scripts/worktree.sh against a scratch repository (RD-140-22): `new` links web/node_modules and
# web/dist from the main checkout, `check` catches generated declarations rewritten through that
# link, and `finish` merges only a branch whose HEAD a green run recorded, discarding the link
# damage and refusing uncommitted work.
#
# Pure git and bash, no cargo.
#
#   scripts/tests/worktree.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
# shellcheck source=../lib/verified.sh
source "$ROOT/scripts/lib/verified.sh"

export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
export CARGO_TARGET_DIR="$SCRATCH/target"

MAIN="$SCRATCH/repo"
git init -q -b development "$MAIN"
mkdir -p "$MAIN/scripts/lib" "$MAIN/web/node_modules" "$MAIN/web/dist"
cp "$ROOT/scripts/worktree.sh" "$MAIN/scripts/"
cp "$ROOT/scripts/lib/verified.sh" "$ROOT/scripts/lib/lanes.sh" "$MAIN/scripts/lib/"
echo 'export {}' > "$MAIN/web/auto-imports.d.ts"
echo 'export {}' > "$MAIN/web/components.d.ts"
printf 'node_modules/\ndist/\n' > "$MAIN/web/.gitignore"
git -C "$MAIN" add -A
git -C "$MAIN" commit -qm base
worktree() { run_status "$MAIN/scripts/worktree.sh" "$@"; }

BRANCH="feat/thing"
TREE="$MAIN-feat-thing"

worktree new "$BRANCH"
expect_status "new creates the worktree" 0
expect "beside the main checkout, named after the branch" "$BRANCH" "$(git -C "$TREE" branch --show-current 2> /dev/null)"
expect "web/node_modules links to the main checkout's" "$MAIN/web/node_modules" "$(readlink "$TREE/web/node_modules")"
expect "web/dist too" "$MAIN/web/dist" "$(readlink "$TREE/web/dist")"

worktree check "$BRANCH"
expect_status "check passes an untouched tree" 0
echo '// rewritten through the link' >> "$TREE/web/components.d.ts"
worktree check "$BRANCH"
expect_status "check refuses rewritten declarations" 1
expect_output "and says how to discard them" "git -C $TREE checkout --"

echo change > "$TREE/feature.txt"
git -C "$TREE" add feature.txt
git -C "$TREE" commit -qm feature
worktree finish "$BRANCH"
expect_status "finish refuses a branch no green run has seen" 1
expect_true "the worktree stays" '[[ -d "$TREE" ]]'

rd_record_verified "$TREE" "$(git -C "$TREE" rev-parse HEAD)"
echo dirty >> "$TREE/feature.txt"
worktree finish "$BRANCH"
expect_status "finish refuses uncommitted changes" 1
git -C "$TREE" checkout -q -- feature.txt

echo '// rewritten again' >> "$TREE/web/components.d.ts"
worktree finish "$BRANCH"
expect_status "finish merges a verified branch" 0
expect "into the base" "change" "$(git -C "$MAIN" show development:feature.txt)"
expect "as a merge commit" "2" "$(git -C "$MAIN" log -1 --format=%p | wc -w | tr -d ' ')"
expect "without the link damage" "export {}" "$(git -C "$MAIN" show development:web/components.d.ts)"
expect_true "the worktree is gone" '[[ ! -e "$TREE" ]]'
expect "and so is the branch" "" "$(git -C "$MAIN" branch --list "$BRANCH")"

worktree finish "$BRANCH"
expect_status "finish of a worktree that does not exist" 1
worktree bogus "$BRANCH"
expect_status "an unknown command is a usage error" 2

finish_tests worktree
