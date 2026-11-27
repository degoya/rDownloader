#!/usr/bin/env bash
#
# scripts/ci-cache.sh stale (RD-1490-02 Nr. 24): of each ref's cache family only the newest stays —
# a family being the key without its trailing hashes and versions — and nothing else is named.
#
#   scripts/tests/ci-cache.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

# <id> <key> <ref> <created_at>: one cache object as the API's `actions_caches[]` gives it.
cache() {
    printf '{"id": %s, "key": "%s", "ref": "%s", "created_at": "%s", "size_in_bytes": 1}\n' "$@"
}

main=refs/heads/main
caches="$(
    cache 1 v0-rust-rust-tests-Linux-x64-87175725-75510178 "$main" 2026-11-22T20:00:00Z
    cache 2 v0-rust-rust-tests-Linux-x64-87175725-f3f42a52 "$main" 2026-11-26T09:16:00Z
    cache 3 v0-rust-rust-tests-Linux-x64-11111111-00000000 "$main" 2026-11-20T09:00:00Z
    cache 4 v0-rust-rust-tests-Windows_NT-x64-35e083e4-f3f42a52 "$main" 2026-11-26T09:14:00Z
    cache 5 v0-rust-rust-tests-Linux-x64-87175725-75510178 refs/heads/development 2026-11-22T20:00:00Z
    cache 6 components-1.100.0-wt1.261.0-c870aaea36761-83f22cdde312 "$main" 2026-11-26T09:04:00Z
    cache 7 components-1.99.0-wt1.260.0-aaaaaaaaaaaa-bbbbbbbbbbbb "$main" 2026-11-01T09:04:00Z
    cache 8 node-cache-Linux-x64-pnpm-b316b9782a907a1a "$main" 2026-11-26T09:03:00Z
    cache 9 node-cache-Linux-arm64-pnpm-b316b9782a907a1a "$main" 2026-11-26T09:03:00Z
    cache 10 docker.io--tonistiigi--binfmt-latest-linux-x64 refs/tags/v1.48.0 2026-11-26T09:18:00Z
)"

run_status "$ROOT/scripts/ci-cache.sh" stale <<< "$caches"
expect_status "stale reads the stream" 0
expect "the older generations go, oldest first within a family, and nothing else" \
    "7 components-1.99.0-wt1.260.0-aaaaaaaaaaaa-bbbbbbbbbbbb|3 v0-rust-rust-tests-Linux-x64-11111111-00000000|1 v0-rust-rust-tests-Linux-x64-87175725-75510178" \
    "$(paste -sd'|' <<< "$output")"
run_status "$ROOT/scripts/ci-cache.sh" stale < /dev/null
expect "no caches, nothing to delete" "" "$output"
run_status "$ROOT/scripts/ci-cache.sh" prune only-one-argument
expect_status "prune wants a repository and a ref" 2

# The workflow that runs it: after both workflows that save, on the two refs that save, with the
# one permission more it needs.
prune_yml="$ROOT/.github/workflows/ci-cache-prune.yml"
expect "it follows CI and Soak" "workflows: [CI, Soak]" "$(grep -o 'workflows: \[.*\]' "$prune_yml")"
expect "on main and development" "branches: [main, development]" "$(grep -o 'branches: \[.*\]' "$prune_yml")"
expect "and may delete caches" "actions: write" "$(grep -o 'actions: write' "$prune_yml")"

finish_tests "ci-cache"
