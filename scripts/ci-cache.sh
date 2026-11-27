#!/usr/bin/env bash
#
# One generation of each Actions cache (RD-1490-02 Nr. 24). GitHub caches are immutable, so a new
# `Cargo.lock`, toolchain or lock file of pnpm saves a new key beside the old one, and the old one
# stays until it has gone unread for seven days: on 2026-11-25 two generations of the Rust caches
# held 11.24 GB against the 9 GB meant to stay (docs/architecture.md#build, "CI's caches"), and at
# 10 GB GitHub evicts what it likes. Every save happens on `main` or `development`; after the
# saving jobs of a push there, ci.yml's `cache-prune` deletes, per ref, every cache older than the
# newest of its family. A family is the key without its trailing hashes and versions
# (`v0-rust-rust-tests-Linux-x64-87175725-f3f42a52` -> `v0-rust-rust-tests-Linux-x64`,
# `components-1.100.0-wt1.261.0-<deps>-<sources>` -> `components`): a newer one is what every run
# restores, through the exact key or the prefix.
#
#   scripts/ci-cache.sh prune <owner/repository> <ref>   # CI only: GH_TOKEN with `actions: write`
#   scripts/ci-cache.sh stale < caches                   # the API's cache objects in, "<id> <key>" out
set -euo pipefail

# The cache objects on stdin (a stream, as `gh api --paginate --jq '.actions_caches[]'` writes
# them): each one an older member of its ref and family, as "<id> <key>", oldest first.
stale() {
    jq -rs '
        def family: split("-")
            | until(length <= 1
                or (last | test("^([0-9a-f]{8,}|(wt)?[0-9]+([.][0-9]+)+)$") | not); .[:-1])
            | join("-");
        map(. + {family: (.key | family)})
        | group_by([.ref, .family])[]
        | sort_by(.created_at, .id)
        | .[:-1][]
        | "\(.id) \(.key)"'
}

prune() {
    local repository="$1" ref="$2" id key
    gh api --paginate "repos/${repository}/actions/caches?per_page=100&ref=${ref}" \
        --jq '.actions_caches[]' | stale | while read -r id key; do
        echo "ci-cache: deleting ${key} (${ref})"
        gh api --method DELETE "repos/${repository}/actions/caches/${id}" --silent \
            || echo "::warning::ci-cache: ${key} was not deleted"
    done
}

case "${1:-}" in
    prune) [[ $# -eq 3 ]] || { echo "usage: scripts/ci-cache.sh prune <owner/repository> <ref>" >&2; exit 2; }
        prune "$2" "$3" ;;
    stale) [[ $# -eq 1 ]] || { echo "usage: scripts/ci-cache.sh stale < caches" >&2; exit 2; }
        stale ;;
    *) echo "usage: scripts/ci-cache.sh prune <owner/repository> <ref> | stale < caches" >&2; exit 2 ;;
esac
