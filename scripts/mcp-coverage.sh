#!/usr/bin/env bash
#
# Writes the MCP coverage comparison into crates/rd-api/mcp-coverage.md.
#
# The comparison is generated rather than kept by hand, because a hand-kept one is wrong by the
# time it is committed. Its source is `crates/rd-api/src/mcp/coverage.rs`: one row per
# capability, the operation counts read from the OpenAPI document utoipa builds in process, the
# tool names read from `mcp::TOOL_POLICY`, the decision and its reason read from the table.
#
# Two tests keep it honest and neither is run by this script:
#   * `coverage::tests::every_documented_operation_belongs_to_a_capability` fails the build when
#     a REST route belongs to no capability, so a new route cannot arrive undecided.
#   * `coverage::doc_tests::the_doc_carries_the_generated_table` fails when the page has
#     drifted from the table, and names this script.
#
# What this deliberately does not do: decide anything. Taking a capability into the toolbox or
# leaving it out is an edit to `coverage.rs` and to the tools beside it; this only transcribes
# the result.
#
# Usage:
#   scripts/mcp-coverage.sh            # rewrite the page's generated block
#   scripts/mcp-coverage.sh --check    # fail if it would change anything
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Sourced before the `cd`, because the lock library resolves this script's own path from $0.
# shellcheck source=lib/lock.sh
source "$ROOT/scripts/lib/lock.sh"
rd_take_lock "$@"
cd "$ROOT"

check_only=0
for argument in "$@"; do
    case "$argument" in
        --check) check_only=1 ;;
        *) echo "unknown argument: $argument" >&2; exit 2 ;;
    esac
done

PAGE="crates/rd-api/mcp-coverage.md"
TEST="mcp::coverage::doc_tests::write_the_doc_table"

# rd-api's integration binaries OOM this workspace when they are all built at once, so the run
# is pinned to the library and to two jobs. See AGENTS.md.
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"

if [[ $check_only -eq 1 ]]; then
    # The ordinary comparison test is the check; it needs no write permission and names this
    # script itself when it fails.
    cargo test -p rd-api --lib -- --exact --nocapture \
        mcp::coverage::doc_tests::the_doc_carries_the_generated_table
    echo "==> $PAGE carries the current comparison"
    exit 0
fi

before="$(sha256sum "$PAGE" | cut -d' ' -f1)"
cargo test -p rd-api --lib -- --ignored --exact --nocapture "$TEST"
after="$(sha256sum "$PAGE" | cut -d' ' -f1)"

if [[ "$before" == "$after" ]]; then
    echo "==> $PAGE was already current"
else
    echo "==> rewrote the comparison in $PAGE"
fi
