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
cleanup() {
    [[ -z "$holder" ]] || kill "$holder" 2> /dev/null || true
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
cp "$ROOT/scripts/lib/"{lock,lanes,verified}.sh "$TREE/scripts/lib/"
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
flock -o "$lockfile" sleep 60 &
holder=$!
for _ in $(seq 1 50); do
    flock -n "$lockfile" true 2> /dev/null || break
    sleep 0.1
done

run_status "$TREE/scripts/prune-target.sh" --if-free
expect_status "the lock held by a build: --if-free exits 0" 0
expect_output "saying why" "a build holds its lock; not pruned (--if-free)"
expect_true "nothing was removed" '[[ -f "$old" && -f "$new" ]]'

kill "$holder" 2> /dev/null || true
wait "$holder" 2> /dev/null || true
holder=""

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

finish_tests prune-target
