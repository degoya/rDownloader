#!/usr/bin/env bash
#
# scripts/release-branches.sh against two scratch repositories standing in for the wiki and the
# website (RD-1270-16): `new` makes the worktree `<repo>-X.Y` on `release-X.Y` from main and is a
# no-op the second time, `status` reports a branch ahead of main (exit 1, `--quiet` says nothing
# else), `ff` moves main by fast-forward only — refusing a diverged main, a dirty release worktree
# and a dirty main checkout before either repository moves — and works without a main checkout.
#
# Pure git and bash: it runs in a second.
#
#   scripts/tests/release-branches.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$ROOT/scripts/release-branches.sh"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
export RD_WIKI_SRC="$SCRATCH/wiki" RD_WEBSITE_DIR="$SCRATCH/site"

for repo in "$RD_WIKI_SRC" "$RD_WEBSITE_DIR"; do
    git init -q -b main "$repo"
    echo one > "$repo/page.md"
    git -C "$repo" add page.md
    git -C "$repo" commit -qm one
done
commit() { echo "$2" >> "$1/page.md"; git -C "$1" commit -qam "$2"; }
rb() { run_status "$SCRIPT" "$@"; }

rb new 1.27
expect_status "new makes both worktrees" 0
expect "the wiki's worktree is on release-1.27" release-1.27 "$(git -C "$SCRATCH/wiki-1.27" branch --show-current)"
expect "the site's worktree is on release-1.27" release-1.27 "$(git -C "$SCRATCH/site-1.27" branch --show-current)"
rb new 1.27.0
expect_status "new again, given the release version, is a no-op" 0
expect_output "and says where the branch is" "release-1.27 is already checked out in $SCRATCH/wiki-1.27"

rb status 1.27
expect_status "a fresh release branch is not ahead" 0
rb status 1.27 --quiet
expect "--quiet says nothing when all is well" "" "$output"

commit "$SCRATCH/wiki-1.27" "handbook for 1.27"
rb status 1.27.0-beta.1 --quiet
expect_status "a release branch with work is ahead" 1
expect_output "status names it and the fix" "release-1.27 is 1 commit(s) ahead of main"
expect_output "and the command" "scripts/release-branches.sh ff 1.27"

echo draft >> "$SCRATCH/wiki-1.27/page.md"
rb ff 1.27
expect_status "ff refuses a dirty release worktree" 1
expect_output "naming it" "$SCRATCH/wiki-1.27 has uncommitted changes"
expect "and moves no main" 1 "$(git -C "$SCRATCH/wiki" rev-list --count main)"
git -C "$SCRATCH/wiki-1.27" checkout -q -- page.md

commit "$SCRATCH/site" "hotfix on main"
rb ff 1.27
expect_status "ff refuses a main with commits the release branch lacks" 1
expect_output "with the reason" "main has 1 commit(s) release-1.27 lacks"
expect_output "and says nothing moved" "nothing moved"
expect "the wiki's main did not move either" 1 "$(git -C "$SCRATCH/wiki" rev-list --count main)"
git -C "$SCRATCH/site-1.27" merge -q --ff-only main

echo local > "$SCRATCH/wiki/scratch.txt"
rb ff 1.27
expect_status "ff refuses a dirty main checkout" 1
expect_output "naming it" "$SCRATCH/wiki (on main) has uncommitted changes"
rm "$SCRATCH/wiki/scratch.txt"

rb ff 1.27
expect_status "ff moves both" 0
expect "the wiki's main is the release branch" "$(git -C "$SCRATCH/wiki" rev-parse release-1.27)" "$(git -C "$SCRATCH/wiki" rev-parse main)"
expect "and its checkout followed" "$(git -C "$SCRATCH/wiki" rev-parse main)" "$(git -C "$SCRATCH/wiki" rev-parse HEAD)"
expect_true "its working tree too" 'grep -q "handbook for 1.27" "$SCRATCH/wiki/page.md"'
expect_output "the site was already there" "main is already at release-1.27"
rb status 1.27
expect_status "after ff nothing is ahead" 0

git -C "$SCRATCH/site" checkout -q --detach
commit "$SCRATCH/site-1.27" "site text for 1.27"
rb ff 1.27
expect_status "ff without a checkout on main moves the ref" 0
expect "the site's main is the release branch" "$(git -C "$SCRATCH/site" rev-parse release-1.27)" "$(git -C "$SCRATCH/site" rev-parse main)"

rb ff 1.28
expect_status "ff of a release line without a branch is refused" 1
expect_output "naming the command that makes it" "no branch release-1.28 (scripts/release-branches.sh new 1.28)"
mkdir "$SCRATCH/wiki-1.28"
rb new 1.28
expect_status "new refuses a directory in the way" 1
expect_output "naming it" "$SCRATCH/wiki-1.28 exists and is not the worktree of release-1.28"
rb new 1.x
expect_status "a malformed release line is a usage error" 2
rb frobnicate 1.27
expect_status "an unknown command is a usage error" 2
RD_WIKI_SRC="$SCRATCH/missing" rb status 1.27
expect_status "status skips a missing repository" 0

finish_tests release-branches
