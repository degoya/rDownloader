#!/usr/bin/env bash
#
# Two decisions of RD-140-06 against a throwaway repository:
#
#  * the boundary a scoped run diffs against (lib/scope.sh, rd_scope_boundary) — the branch point,
#    an older green, and now a green on the branch itself, which a follow-up round starts from;
#  * whether the release chain may skip its full Rust run (lib/verified.sh,
#    rd_prebump_full_green) — only for a --full green of HEAD's tree and a bump that changed
#    nothing but version lines.
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
git add -A
git commit -qm "release base"
tree="$(git rev-parse 'HEAD^{tree}')"
bump() {
    sed -i 's/^version = "1\.0\.0"/version = "1.1.0"/' Cargo.toml Cargo.lock
    sed -i 's/"version": "1\.0\.0"/"version": "1.1.0"/' web/package.json extension/manifest.base.json
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
bump
rd_record_full "$repo" rust "$(git rev-parse 'HEAD~1^{tree}')"
expect "a --full green of another tree: run" "" "$(rd_prebump_full_green "$repo")"
rd_record_full "$repo" web "$tree"
expect "a --full green of the web half only: run" "" "$(rd_prebump_full_green "$repo")"

echo
if [[ "$failures" -gt 0 ]]; then
    echo "verified-scope: $failures failed, $passed passed"
    exit 1
fi
echo "verified-scope: $passed passed"
