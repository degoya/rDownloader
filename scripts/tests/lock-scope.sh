#!/usr/bin/env bash
#
# The Cargo.toml/Cargo.lock scope rules (RD-130-17) against hand-written fixtures.
#
# Each case under fixtures/lock-scope/cases/ lays its old/ and new/ files over the same base
# workspace, runs scripts/lib/lock-scope.py on the two trees and compares stdout with its
# `expected` file, byte for byte. Three cases are the widened branch checks of 2026-09-24
# (dev-dependency-added, registry-version-bump, new-workspace-dependency); the rest pin the
# answers that must stay WIDE and the ones that must not. The path rule check.sh keeps for
# itself — the toolchain, nextest and deny — is tested last, through lib/scope.sh.
#
# Pure python3 and bash, no cargo: it runs in a second. check.sh runs it when scripts/lib/ or
# scripts/tests/ change, and under --full.
#
#   scripts/tests/lock-scope.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FIXTURES="$ROOT/scripts/tests/fixtures/lock-scope"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

failures=0
passed=0

for case_dir in "$FIXTURES"/cases/*/; do
    name="$(basename "$case_dir")"
    rm -rf "$SCRATCH/old" "$SCRATCH/new"
    for side in old new; do
        cp -R "$FIXTURES/base" "$SCRATCH/$side"
        if [[ -d "$case_dir$side" ]]; then cp -R "$case_dir$side/." "$SCRATCH/$side/"; fi
    done
    if ! actual="$(python3 "$ROOT/scripts/lib/lock-scope.py" "$SCRATCH/old" "$SCRATCH/new" 2>&1)"; then
        echo "FAIL $name: lock-scope.py exited non-zero"
        printf '     %s\n' "$actual"
        failures=$((failures + 1))
        continue
    fi
    expected="$(cat "$case_dir/expected")"
    if [[ "$actual" == "$expected" ]]; then
        echo "ok   $name"
        passed=$((passed + 1))
    else
        echo "FAIL $name"
        diff <(printf '%s\n' "$expected") <(printf '%s\n' "$actual") | sed 's/^/     /' || true
        failures=$((failures + 1))
    fi
done

# shellcheck source=../lib/scope.sh
source "$ROOT/scripts/lib/scope.sh"
while read -r path verdict; do
    got="$(rd_scope_build_governing <<< "$path")"
    if [[ ( "$verdict" == wide && "$got" == "$path" ) || ( "$verdict" == narrow && -z "$got" ) ]]; then
        echo "ok   path $path -> $verdict"
        passed=$((passed + 1))
    else
        echo "FAIL path $path: expected $verdict, got '${got}'"
        failures=$((failures + 1))
    fi
done <<'EOF'
rust-toolchain.toml wide
.config/nextest.toml wide
deny.toml wide
Cargo.lock narrow
Cargo.toml narrow
crates/rd-core/Cargo.toml narrow
EOF

echo
if [[ "$failures" -gt 0 ]]; then
    echo "lock-scope: $failures failed, $passed passed"
    exit 1
fi
echo "lock-scope: $passed passed"
