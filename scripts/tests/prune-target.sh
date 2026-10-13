#!/usr/bin/env bash
#
# scripts/prune-target.sh --if-free (RD-160-06) against a scratch target: with the target's lock
# held by a build it prunes nothing, says so and exits 0; with the lock free it prunes the old
# variant and keeps the newest; per stem it keeps the newest variant of each kind of artifact. The
# lock is a scratch file (RD_LOCK_FILE), never the real one.
#
#   scripts/tests/prune-target.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
holder=""
release_holder() {
    pkill -P "$holder" 2> /dev/null || true
    kill "$holder" 2> /dev/null || true
    wait "$holder" 2> /dev/null || true
    holder=""
}
cleanup() {
    [[ -z "$holder" ]] || release_holder
    rm -rf "$SCRATCH"
}
trap cleanup EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

command -v flock > /dev/null || { echo "prune-target: flock is not installed; skipped"; exit 0; }

export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
TREE="$SCRATCH/tree"
mkdir -p "$TREE/scripts/lib"
cp "$ROOT/scripts/prune-target.sh" "$TREE/scripts/"
cp "$ROOT/scripts/lib/"{lock,lanes,verified,verified-ci,workspace-version,inert-paths}.sh "$TREE/scripts/lib/"
git init -q -b development "$TREE"
git -C "$TREE" commit -q --allow-empty -m base

export CARGO_TARGET_DIR="$SCRATCH/target"
export RD_LOCK_FILE="$SCRATCH/rd-build.lock"
unset RD_LOCK_HELD RD_NO_LOCK
deps="$CARGO_TARGET_DIR/debug/deps"
mkdir -p "$deps"
old="$deps/librd_core-0123456789abcdef.rlib"
new="$deps/librd_core-fedcba9876543210.rlib"
echo old > "$old"
echo new > "$new"
touch -d '2 hours ago' "$old"

# shellcheck source=../lib/lanes.sh
source "$ROOT/scripts/lib/lanes.sh"
lockfile="$(rd_target_lock_file "$CARGO_TARGET_DIR" "$TREE")"
# -o: the lock stays with flock itself, so killing it frees the lock (sleep would inherit it).
# The holder writes nowhere: an inherited stdout kept `check.sh`'s `| tee` of this stage open
# until the orphaned sleep ended, 60 s per run (RD-1270-13; lock-lanes.sh does the same).
flock -o "$lockfile" sleep 60 > /dev/null 2>&1 < /dev/null &
holder=$!
for _ in $(seq 1 50); do
    flock -n "$lockfile" true 2> /dev/null || break
    sleep 0.1
done

run_status "$TREE/scripts/prune-target.sh" --if-free
expect_status "the lock held by a build: --if-free exits 0" 0
expect_output "saying why" "a build holds its lock; not pruned (--if-free)"
expect_true "nothing was removed" '[[ -f "$old" && -f "$new" ]]'

release_holder

run_status "$TREE/scripts/prune-target.sh" --if-free
expect_status "the lock free: --if-free prunes" 0
expect_true "the old variant is gone" '[[ ! -e "$old" ]]'
expect_true "the newest is kept" '[[ -f "$new" ]]'
expect_output "and it said what went" "removed 1 files"

# Per stem and kind (RD-160-06, Maßnahme 8): the newest `.d` of a `check` run no longer evicts the
# test binary of the same stem, nor a library's `.rmeta` its build's `.rlib`.
rm -rf "$deps" && mkdir -p "$deps"
binary_old="$deps/rd_core-1111111111111111"
binary="$deps/rd_core-2222222222222222"
checked="$deps/rd_core-3333333333333333.d"
rlib="$deps/librd_core-2222222222222222.rlib"
rmeta="$deps/librd_core-3333333333333333.rmeta"
for file in "$binary_old" "$binary_old.d" "$binary" "$binary.d" "$checked" "$rlib" \
    "$deps/librd_core-2222222222222222.rmeta" "$rmeta"; do
    echo x > "$file"
done
touch -d '3 hours ago' "$binary_old" "$binary_old.d"
touch -d '1 hour ago' "$binary" "$binary.d" "$rlib" "$deps/librd_core-2222222222222222.rmeta"

run_status "$TREE/scripts/prune-target.sh"
expect_status "a target with a check newer than the build: pruned" 0
expect_true "the newest test binary stays beside a newer check's .d" '[[ -f "$binary" && -f "$binary.d" ]]'
expect_true "the check's .d stays" '[[ -f "$checked" ]]'
expect_true "the build's .rlib stays beside a newer check's .rmeta" '[[ -f "$rlib" && -f "$rmeta" ]]'
expect_true "the older test binary goes" '[[ ! -e "$binary_old" && ! -e "$binary_old.d" ]]'
expect_output "and only it" "removed 2 files"

# --if-free never fails without a reason (RD-1270-13). integrate.sh recorded `prune=1` with an
# empty log: the target list ended in a linked worktree with a `target/` that is not its own,
# rd_all_target_dirs returned that test's 1, and errexit ended the run without a word.
git -C "$TREE" worktree add -q "$SCRATCH/linked"
mkdir -p "$SCRATCH/linked/target"
run_status "$TREE/scripts/prune-target.sh" --if-free
expect_status "a linked worktree sharing the target is listed last: --if-free still prunes" 0
expect_output "and reports its own target" "$deps: removed"

# A step that fails without a word is still named, with its exit code, by the run and its parent.
shim="$SCRATCH/shim"
mkdir -p "$shim"
printf '#!/usr/bin/env bash\nexit 3\n' > "$shim/python3"
chmod +x "$shim/python3"
run_status env PATH="$shim:$PATH" "$TREE/scripts/prune-target.sh" --if-free
expect_true "a silent failure fails the run" '[[ $status -ne 0 ]]'
expect_output "naming the command that failed" "prune-target: failed (exit 3) at"
expect_output "and the target it gave up on" "$CARGO_TARGET_DIR: not pruned (exit 3"

finish_tests prune-target
