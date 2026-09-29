#!/usr/bin/env bash
#
# scripts/worktree.sh against a scratch repository (RD-140-22): `new` installs web/node_modules
# with pnpm and links web/dist from the main checkout (RD-150-14), `check` catches rewritten
# generated declarations, and `finish` merges only a branch whose HEAD a green run recorded,
# discarding such a rewrite and refusing uncommitted work; a branch already in the base is only
# removed.
#
# Pure git and bash, no cargo; pnpm is a stand-in that records its arguments.
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

mkdir -p "$SCRATCH/bin"
cat > "$SCRATCH/bin/pnpm" <<'STUB'
#!/usr/bin/env bash
echo "$*" >> "$(dirname "$0")/pnpm.log"
[[ "$1" == install && "$2" == --dir ]] && mkdir -p "$3/node_modules"
exit 0
STUB
chmod +x "$SCRATCH/bin/pnpm"
export PATH="$SCRATCH/bin:$PATH"

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
expect "web/node_modules is installed from the lockfile" "install --dir $TREE/web --frozen-lockfile" "$(cat "$SCRATCH/bin/pnpm.log")"
expect_true "as a directory of its own, not a link" '[[ -d "$TREE/web/node_modules" && ! -L "$TREE/web/node_modules" ]]'
expect "web/dist links to the main checkout's" "$MAIN/web/dist" "$(readlink "$TREE/web/dist")"

worktree check "$BRANCH"
expect_status "check passes an untouched tree" 0
echo '// rewritten by a build' >> "$TREE/web/components.d.ts"
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
expect "without the rewrite" "export {}" "$(git -C "$MAIN" show development:web/components.d.ts)"
expect_true "the worktree is gone, its node_modules with it" '[[ ! -e "$TREE" ]]'
expect_true "the main checkout's web/dist is untouched" '[[ -d "$MAIN/web/dist" ]]'
expect "and so is the branch" "" "$(git -C "$MAIN" branch --list "$BRANCH")"

worktree finish "$BRANCH"
expect_status "finish of a worktree that does not exist" 1

# A wave branch: merged into the base through its integration branch, never checked on its own.
WAVE="wave/part"
WAVE_TREE="$MAIN-wave-part"
worktree new "$WAVE"
echo part > "$WAVE_TREE/part.txt"
git -C "$WAVE_TREE" add part.txt
git -C "$WAVE_TREE" commit -qm part
git -C "$MAIN" merge -q --no-ff -m integrate "$WAVE"
echo dirty >> "$WAVE_TREE/part.txt"
worktree finish "$WAVE"
expect_status "a merged branch with uncommitted changes is still refused" 1
git -C "$WAVE_TREE" checkout -q -- part.txt
worktree finish "$WAVE"
expect_status "a branch already in the base is finished without a green of its own" 0
expect_true "its worktree is gone" '[[ ! -e "$WAVE_TREE" ]]'
expect "and its branch" "" "$(git -C "$MAIN" branch --list "$WAVE")"
expect "no second merge commit" "integrate" "$(git -C "$MAIN" log -1 --format=%s)"
worktree bogus "$BRANCH"
expect_status "an unknown command is a usage error" 2

finish_tests worktree
