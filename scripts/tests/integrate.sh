#!/usr/bin/env bash
#
# The merge half of scripts/integrate.sh against a scratch repository (RD-140-22), with
# --merge-only: nothing here compiles. The integration worktree is created from the base, each
# branch merged once, a duplicate migration number or plugin id stops the run naming the files,
# a conflict only in generated files takes our side, and any other conflict stops the run with
# the merge left for a person — and a second run refuses until it is committed.
#
#   scripts/tests/integrate.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
export CARGO_TARGET_DIR="$SCRATCH/target"
# worktree.sh installs web/node_modules with pnpm (RD-150-14); nothing here needs a real one.
mkdir -p "$SCRATCH/bin"
printf '#!/usr/bin/env bash\nexit 0\n' > "$SCRATCH/bin/pnpm"
chmod +x "$SCRATCH/bin/pnpm"
export PATH="$SCRATCH/bin:$PATH"

MAIN="$SCRATCH/repo"
git init -q -b development "$MAIN"
mkdir -p "$MAIN/scripts/lib" "$MAIN/web/node_modules" "$MAIN/web/dist" "$MAIN/crates/rd-db/migrations"
cp "$ROOT/scripts/integrate.sh" "$ROOT/scripts/worktree.sh" "$MAIN/scripts/"
cp "$ROOT/scripts/lib/integrate.sh" "$ROOT/scripts/lib/verified.sh" "$ROOT/scripts/lib/lanes.sh" \
    "$ROOT/scripts/lib/workspace-version.sh" "$MAIN/scripts/lib/"
printf 'node_modules/\ndist/\n' > "$MAIN/web/.gitignore"
echo '{"version": "base"}' > "$MAIN/web/openapi.json"
echo 'base' > "$MAIN/shared.txt"
echo 'CREATE TABLE a (id INTEGER);' > "$MAIN/crates/rd-db/migrations/0001_a.sql"
git -C "$MAIN" add -A
git -C "$MAIN" commit -qm base

# A branch from development with one commit that writes $2 into file $3.
branch() {
    git -C "$MAIN" checkout -q -b "$1" development
    mkdir -p "$(dirname "$MAIN/$3")"
    printf '%s\n' "$2" > "$MAIN/$3"
    git -C "$MAIN" add -A
    git -C "$MAIN" commit -qm "$1"
    git -C "$MAIN" checkout -q development
}
branch feat/one 'one' one.txt
branch feat/two 'two' two.txt
branch feat/migration 'CREATE TABLE b (id INTEGER);' crates/rd-db/migrations/0002_b.sql
branch feat/same-number 'CREATE TABLE c (id INTEGER);' crates/rd-db/migrations/0002_c.sql
branch feat/plugin-a 'id = "019d-0160"' plugins/a/manifest.toml
branch feat/plugin-b 'id = "019d-0160"' plugins/b/manifest.toml
branch feat/api-one '{"version": "one"}' web/openapi.json
branch feat/api-two '{"version": "two"}' web/openapi.json
branch feat/edit-one 'edited by one' shared.txt
branch feat/edit-two 'edited by two' shared.txt

integrate() { run_status "$MAIN/scripts/integrate.sh" "$@" --merge-only; }
TREE="$MAIN-integration-w1"

integrate integration/w1 feat/one feat/two feat/migration
expect_status "three clean branches" 0
expect "the integration branch lives in its own worktree" "integration/w1" "$(git -C "$TREE" branch --show-current)"
expect_true "and carries all three" 'for b in feat/one feat/two feat/migration; do git -C "$TREE" merge-base --is-ancestor "$b" HEAD || exit 1; done'
expect "each as a merge commit" "3" "$(git -C "$TREE" rev-list --merges --count development..HEAD)"
expect_output "no duplicates" "no duplicate migration number, no duplicate plugin id"
expect_true "the main checkout is not moved" '[[ "$(git -C "$MAIN" branch --show-current)" == development ]]'

integrate integration/w1 feat/one feat/two feat/migration
expect_status "a second run" 0
expect_output "skips what is merged" "feat/one: already merged"
expect "and merges nothing again" "3" "$(git -C "$TREE" rev-list --merges --count development..HEAD)"

integrate integration/w1 feat/same-number
expect_status "a second migration 0002 stops the run" 1
expect_output "naming both files" "duplicate migration number 0002: 0002_b.sql 0002_c.sql"
git -C "$TREE" reset -q --hard HEAD^

integrate integration/w1 feat/plugin-a feat/plugin-b
expect_status "a second plugin with the same id stops the run" 1
expect_output "naming both manifests" 'duplicate plugin id = "019d-0160": plugins/a/manifest.toml plugins/b/manifest.toml'
git -C "$TREE" reset -q --hard HEAD^

integrate integration/w1 feat/api-one feat/api-two
expect_status "a conflict only in a generated file is resolved" 0
expect_output "and says the generators rewrite it" "generated files in conflict took our side"
expect "our side, which the generators then replace" '{"version": "one"}' "$(cat "$TREE/web/openapi.json")"

integrate integration/w1 feat/edit-one feat/edit-two
expect_status "a conflict in a source file stops the run" 1
expect_output "naming the file" "shared.txt"
expect_true "the merge is left in progress for a person" 'git -C "$TREE" rev-parse --verify --quiet MERGE_HEAD > /dev/null'
integrate integration/w1 feat/edit-two
expect_status "a run while the merge is unresolved is refused" 1
expect_output "saying why" "a merge is in progress"

integrate integration/w1 feat/nope
expect_status "a branch that does not exist" 2
run_status "$MAIN/scripts/integrate.sh" integration/w1
expect_status "no branch to merge is a usage error" 2

finish_tests integrate
