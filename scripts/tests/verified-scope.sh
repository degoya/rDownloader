#!/usr/bin/env bash
#
# Two decisions of RD-140-06 against a throwaway repository:
#
#  * the boundary a scoped run diffs against (lib/scope.sh, rd_scope_boundary) — the branch point,
#    an older green, and now a green on the branch itself, which a follow-up round starts from;
#  * whether the release chain may skip its full Rust run (lib/verified.sh,
#    rd_prebump_full_green) — only for a --full green of HEAD's tree and a bump that changed
#    nothing but version lines;
#  * whether check.sh --full runs at all (rd_full_covering, rd_full_already_green, RD-160-06) — not
#    for content a --full green of any checkout on the target covers up to documentation;
#  * which GitHub platforms a tree still needs (rd_ci_covering, rd_tree_same_but_versions) — none
#    that a green covers up to documentation and version lines.
#
# Pure git and bash, no cargo: it runs in a second. check.sh runs it when scripts/lib/ or
# scripts/tests/ change, and under --full.
#
#   scripts/tests/verified-scope.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

# shellcheck source=../lib/verified.sh
source "$ROOT/scripts/lib/verified.sh"
# shellcheck source=../lib/scope.sh
source "$ROOT/scripts/lib/scope.sh"

failures=0
passed=0
expect() {
    local name="$1" expected="$2" actual="$3"
    if [[ "$actual" == "$expected" ]]; then
        echo "ok   $name"
        passed=$((passed + 1))
    else
        echo "FAIL $name: expected '${expected}', got '${actual}'"
        failures=$((failures + 1))
    fi
}

repo="$SCRATCH/repo"
export CARGO_TARGET_DIR="$SCRATCH/target"
export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
git init -q -b development "$repo"
cd "$repo"
commit() { printf '%s\n' "$1" > "file-$1"; git add -A; git commit -qm "$1"; git rev-parse HEAD; }

# --- the scope boundary -------------------------------------------------------------------------
c1="$(commit c1)"
c2="$(commit c2)"
git checkout -q -b other
o1="$(commit o1)"
git checkout -q development
git checkout -q -b feature
f1="$(commit f1)"
f2="$(commit f2)"

rm -rf "$CARGO_TARGET_DIR"
expect "no green: the branch point" "$c2" "$(rd_scope_boundary development "$repo")"
rd_record_verified "$repo" "$c1"
expect "a green older than the branch point: the green" "$c1" "$(rd_scope_boundary development "$repo")"
rd_record_verified "$repo" "$f1"
expect "a green on this branch: the green (a follow-up round checks the delta)" "$f1" "$(rd_scope_boundary development "$repo")"
rd_record_verified "$repo" "$f2"
expect "a green at HEAD: HEAD" "$f2" "$(rd_scope_boundary development "$repo")"
rd_record_verified "$repo" "$o1"
expect "a green HEAD does not contain: the branch point" "$c2" "$(rd_scope_boundary development "$repo")"
rd_record_verified "$repo" "0123456789abcdef0123456789abcdef01234567"
expect "a green git does not know: the branch point" "$c2" "$(rd_scope_boundary development "$repo")"
git checkout -q development
rd_record_verified "$repo" "$c1"
expect "on development itself: the last green" "$c1" "$(rd_scope_boundary development "$repo")"

# --- the release chain's pre-bump green ---------------------------------------------------------
git checkout -q -b release
mkdir -p web extension
cat > Cargo.toml <<'EOF'
[workspace]
members = ["crates/*"]

[workspace.package]
version = "1.0.0"
edition = "2024"
EOF
cat > Cargo.lock <<'EOF'
[[package]]
name = "rd-core"
version = "1.0.0"

[[package]]
name = "serde"
version = "1.0.200"
EOF
printf '{\n  "name": "web",\n  "version": "1.0.0",\n  "private": true\n}\n' > web/package.json
printf '{\n  "name": "ext",\n  "version": "1.0.0"\n}\n' > extension/manifest.base.json
printf '{\n  "info": {\n    "title": "rd-api",\n    "version": "1.0.0"\n  },\n  "paths": {}\n}\n' > web/openapi.json
git add -A
git commit -qm "release base"
tree="$(git rev-parse 'HEAD^{tree}')"
bump() {
    sed -i 's/^version = "1\.0\.0"/version = "1.1.0"/' Cargo.toml Cargo.lock
    sed -i 's/"version": "1\.0\.0"/"version": "1.1.0"/' web/package.json extension/manifest.base.json web/openapi.json
}
reset() { git checkout -q -- . && git clean -qfd; }

rm -rf "$CARGO_TARGET_DIR"
bump
expect "a bump without any --full green: run" "" "$(rd_prebump_full_green "$repo")"
rd_record_full "$repo" rust "$tree"
expect "a bump of a tree with a --full green: skip, naming the green" "$tree" "$(rd_prebump_full_green "$repo")"
reset
expect "no bump at all: run" "" "$(rd_prebump_full_green "$repo")"
bump
sed -i 's/^edition = "2024"/edition = "2021"/' Cargo.toml
expect "a bump with another line of Cargo.toml: run" "" "$(rd_prebump_full_green "$repo")"
reset
bump
sed -i 's/version = "1\.0\.200"/version = "1.0.201"/' Cargo.lock
expect "a bump that moved a dependency in Cargo.lock: run" "" "$(rd_prebump_full_green "$repo")"
reset
bump
echo change >> file-c1
expect "a bump with another tracked file: run" "" "$(rd_prebump_full_green "$repo")"
reset
bump
echo new > untracked.txt
expect "a bump with an untracked file: run" "" "$(rd_prebump_full_green "$repo")"
reset
# A beta as set-version.sh writes it: the browser manifest takes the version without the suffix.
beta_bump() {
    sed -i 's/^version = "1\.0\.0"/version = "1.1.0-beta.1"/' Cargo.toml Cargo.lock
    sed -i 's/"version": "1\.0\.0"/"version": "1.1.0-beta.1"/' web/package.json web/openapi.json
    sed -i 's/"version": "1\.0\.0"/"version": "1.1.0"/' extension/manifest.base.json
}
beta_bump
expect "a bump to a beta, the browser manifest without the suffix: skip" "$tree" "$(rd_prebump_full_green "$repo")"
reset
beta_bump
sed -i 's/^version = "1\.1\.0-beta\.1"/version = "1.1.0"/' Cargo.lock
expect "a beta bump with a Cargo.lock line at the bare version: run" "" "$(rd_prebump_full_green "$repo")"
reset
bump
rd_record_full "$repo" rust "$(git rev-parse 'HEAD~1^{tree}')"
expect "a --full green of another tree: run" "" "$(rd_prebump_full_green "$repo")"
rd_record_full "$repo" web "$tree"
expect "a --full green of the web half only: run" "" "$(rd_prebump_full_green "$repo")"

# --- a --full green covers the same content, from any checkout (RD-160-06) ---------------------
reset
green="$(git rev-parse 'HEAD^{tree}')"
rm -rf "$CARGO_TARGET_DIR"
other="$SCRATCH/integration-worktree"
rd_record_full "$other" rust "$green"
rd_record_full "$other" web "$green"
expect "another checkout's green of this very tree covers it" "$green" "$(rd_full_covering "$repo" rust "$green")"
mkdir -p docs
echo "verification note" > docs/note.md
echo "## [1.0.0]" > CHANGELOG.md
now="$(rd_worktree_tree "$repo")"
expect "a green of a tree that differs in documentation only covers it" "$green" "$(rd_full_covering "$repo" web "$now")"
set +e
full_again="$(rd_full_already_green "$repo" rust web)"
covered=$?
set -e
expect "check.sh --full is not run again for it" "0" "$covered"
expect_line() { if grep -qF -- "$2" <<< "$3"; then expect "$1" x x; else expect "$1" "$2" "$3"; fi; }
expect_line "and names the green it relies on" "rust: tree ${green:0:12}, documentation changed since" "$full_again"
expect "and records the halves for this tree, so the gates find them" "$now" "$(sed -n 's/^web //p' "$(rd_full_marker "$repo")")"
expect "the gate of the tag and the Windows package agrees" "0" "$(rd_full_gate "$repo" "the test" > /dev/null 2>&1; echo $?)"
reset
echo "change" > crates.txt
expect "a code change is not covered" "" "$(rd_full_covering "$repo" rust "$(rd_worktree_tree "$repo")")"
set +e
rd_full_already_green "$repo" rust > /dev/null
covered=$?
set -e
expect "and check.sh --full runs" "1" "$covered"
reset
mkdir -p crates/rd-core
echo "| point |" > crates/rd-core/recovery-matrix.md
expect "a .md a test reads is not documentation" "" "$(rd_full_covering "$repo" rust "$(rd_worktree_tree "$repo")")"
reset
rm -rf "$CARGO_TARGET_DIR"
rd_record_full "$other" rust "$green"
expect "one half recorded does not cover the other" "" "$(rd_full_covering "$repo" web "$green")"

# check.sh --full itself: with a covering green it ends before the lock, before any cargo. Only
# the skipping side is run here; the other would start the real check — should the skip ever
# break, a lock of its own that is never waited for keeps it away from the real one.
wired="$SCRATCH/wired"
git init -q -b development "$wired"
mkdir -p "$wired/scripts/lib"
cp "$ROOT/scripts/check.sh" "$wired/scripts/"
cp "$ROOT/scripts/lib/"*.sh "$wired/scripts/lib/"
git -C "$wired" add -A
git -C "$wired" commit -qm "the checked state"
rm -rf "$CARGO_TARGET_DIR"
rd_record_full "$SCRATCH/elsewhere" rust "$(git -C "$wired" rev-parse 'HEAD^{tree}')"
rd_record_full "$SCRATCH/elsewhere" web "$(git -C "$wired" rev-parse 'HEAD^{tree}')"
echo "note" > "$wired/NOTES.md"
set +e
wired_output="$(env -u RD_LOCK_HELD RD_LOCK_FILE="$SCRATCH/lock" RD_LOCK_WAIT=0 "$wired/scripts/check.sh" --full 2>&1)"
covered=$?
set -e
expect "check.sh --full over a covered tree: passes without running" "0" "$covered"
expect_line "saying so" "==> all requested checks passed" "$wired_output"
expect "and records the revision as verified" "$(git -C "$wired" rev-parse HEAD)" "$(rd_verified_revision "$wired")"
rm -f "$(rd_verified_marker "$wired")"
set +e
wired_output="$(env -u RD_LOCK_HELD RD_LOCK_FILE="$SCRATCH/lock" RD_LOCK_WAIT=0 "$wired/scripts/check.sh" --rust --full 2>&1)"
covered=$?
set -e
expect "check.sh --rust --full over a covered tree: passes" "0" "$covered"
expect "a half run records no verified revision" "" "$(rd_verified_revision "$wired")"
rm -rf "$CARGO_TARGET_DIR"
rd_record_full "$SCRATCH/elsewhere" windows "$(git -C "$wired" rev-parse 'HEAD^{tree}')"
set +e
wired_output="$(env -u RD_LOCK_HELD RD_LOCK_FILE="$SCRATCH/lock" RD_LOCK_WAIT=0 "$wired/scripts/check.sh" --windows 2>&1)"
covered=$?
set -e
expect "check.sh --windows over a tree its green covers up to documentation: passes" "0" "$covered"
expect_line "saying it is not run again" "(windows); it is not run again" "$wired_output"
expect "and records the Windows green for this tree" "$(rd_worktree_tree "$wired")" "$(sed -n 's/^windows //p' "$(rd_full_marker "$wired")")"
expect "a Windows green covers no half of --full" "" "$(rd_full_covering "$wired" rust "$(rd_worktree_tree "$wired")")"
echo "change" > "$wired/code.rs"
expect "a code change is not covered by the Windows green" "" "$(rd_full_covering "$wired" windows "$(rd_worktree_tree "$wired")")"
rm -f "$wired/code.rs"
# The gate (RD-1100-13): its two halves, `clippy` and `windows`, kept the same way.
rd_record_full "$SCRATCH/elsewhere" clippy "$(git -C "$wired" rev-parse 'HEAD^{tree}')"
set +e
wired_output="$(env -u RD_LOCK_HELD RD_LOCK_FILE="$SCRATCH/lock" RD_LOCK_WAIT=0 "$wired/scripts/check.sh" --gate 2>&1)"
covered=$?
set -e
expect "check.sh --gate over a tree both its greens cover: passes" "0" "$covered"
expect_line "saying it is not run again" "(clippy windows); it is not run again" "$wired_output"
expect "and records no verified revision, which only --full's two halves do" "" "$(rd_verified_revision "$wired")"

# --- GitHub greens per platform, up to documentation and version lines (RD-160-06) --------------
rm -f "$(rd_ci_record_file "$repo")"
expect "no record: nothing covers a platform" "" "$(rd_ci_covering "$repo" "$green" macos-15)"
rd_record_ci "$repo" "$green" ubuntu-24.04 windows-2025
expect "a recorded platform of this tree is covered" "$green" "$(rd_ci_covering "$repo" "$green" windows-2025)"
expect "an unrecorded platform is not" "" "$(rd_ci_covering "$repo" "$green" macos-15)"
bump
echo "## [1.1.0]" > CHANGELOG.md
mkdir -p docs && echo "moved" > docs/archived-job.md
git add -A
git commit -qm "chore(release): 1.1.0"
candidate="$(git rev-parse 'HEAD^{tree}')"
expect "the release candidate: bump and documentation count as the same content" "$green" "$(rd_ci_covering "$repo" "$candidate" ubuntu-24.04)"
expect "but macOS still has to run" "" "$(rd_ci_covering "$repo" "$candidate" macos-15)"
sed -i 's/version = "1\.0\.200"/version = "1.0.201"/' Cargo.lock
git commit -qam "a dependency moved"
expect "a dependency moved in Cargo.lock: not the same content" "" "$(rd_ci_covering "$repo" "$(git rev-parse 'HEAD^{tree}')" ubuntu-24.04)"
git reset -q --hard HEAD~1
echo change >> file-c1
git commit -qam "a code change"
expect "a code change: not the same content" "" "$(rd_ci_covering "$repo" "$(git rev-parse 'HEAD^{tree}')" ubuntu-24.04)"
git reset -q --hard HEAD~1
sed -i 's/^edition = "2024"/edition = "2021"/' Cargo.toml
git commit -qam "another Cargo.toml line"
expect "another line of a version file: not the same content" "" "$(rd_ci_covering "$repo" "$(git rev-parse 'HEAD^{tree}')" ubuntu-24.04)"
git reset -q --hard HEAD~1
echo "verification note" > docs/note.md
git add -A
git commit -qm "docs: the verification note"
rd_record_ci "$repo" "$candidate" macos-15
expect "only documentation after the green: the same content" "$candidate" "$(rd_ci_covering "$repo" "$(git rev-parse 'HEAD^{tree}')" macos-15)"

echo
if [[ "$failures" -gt 0 ]]; then
    echo "verified-scope: $failures failed, $passed passed"
    exit 1
fi
echo "verified-scope: $passed passed"
