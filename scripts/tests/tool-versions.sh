#!/usr/bin/env bash
#
# scripts/tool-versions.sh (RD-1280-12) against copies of the files it reads: this tree's pins
# agree; a copy moved in a workflow, a composite action, the Dockerfile or the documentation, and
# a source moved without its copies, each fail and name the file; a copy no rule finds any more
# fails instead of passing quietly; a tree without docs/ (the public export) passes.
#
#   scripts/tests/tool-versions.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

# A fresh copy of what the check reads.
fresh() {
    rm -rf "$SCRATCH/tree"
    mkdir -p "$SCRATCH/tree"
    (cd "$ROOT" && git ls-files --cached --others --exclude-standard rust-toolchain.toml \
        scripts/lib/tool-versions.sh scripts/lib/plugin-stamp.sh .github sdk/ci docker/Dockerfile \
        AGENTS.md docs/development.md docs/architecture.md docs/reproducible-builds.md sdk/README.md) \
        | while IFS= read -r file; do
            [[ -f "$ROOT/$file" ]] || continue
            mkdir -p "$SCRATCH/tree/$(dirname "$file")"
            cp "$ROOT/$file" "$SCRATCH/tree/$file"
        done
}
check() { run_status "$ROOT/scripts/tool-versions.sh" --check --root "$SCRATCH/tree"; }

run_status "$ROOT/scripts/tool-versions.sh"
expect_status "the listing succeeds" 0
expect_output "it lists nextest" "nextest"

fresh
check
expect_status "this tree's pins agree" 0
expect_output "naming how many copies" "copies of 7 tool pins agree"

fresh
sed -i 's/tool: nextest@[0-9.]*/tool: nextest@0.0.1/' "$SCRATCH/tree/.github/actions/rust-tests-cache/action.yml"
check
expect_status "a moved nextest in a composite action fails" 1
expect_output "naming the file" ".github/actions/rust-tests-cache/action.yml"
expect_output "and both versions" "nextest 0.0.1, the source says"

fresh
sed -i 's/^channel = ".*"/channel = "1.0.0"/' "$SCRATCH/tree/rust-toolchain.toml"
check
expect_status "a Rust moved without its copies fails" 1
expect_output "naming the component cache key" ".github/workflows/ci-components.yml"
expect_output "and the Dockerfile" "docker/Dockerfile"

fresh
sed -i 's/^RD_SQLX_CLI_VERSION=.*/RD_SQLX_CLI_VERSION="9.9.9"/' "$SCRATCH/tree/scripts/lib/tool-versions.sh"
check
expect_status "a moved source in scripts/lib/tool-versions.sh fails" 1
expect_output "naming the workflow that still has the old one" ".github/workflows/ci.yml"

fresh
sed -i 's/^| actionlint | [0-9.]*/| actionlint | 0.1.0/' "$SCRATCH/tree/docs/development.md"
check
expect_status "a moved version in the documentation's list fails" 1
expect_output "naming the document" "docs/development.md"

fresh
sed -i 's/tool: wasm-tools@/tool: wasm-tools-cli@/' "$SCRATCH/tree/sdk/ci/plugin.yml" "$SCRATCH/tree"/.github/workflows/*.yml
check
expect_status "a copy no rule finds any more fails" 1
expect_output "saying the rule has to follow it" "the copy moved"

fresh
rm -rf "$SCRATCH/tree/docs" "$SCRATCH/tree/AGENTS.md"
check
expect_status "a tree without docs/ (the public export) passes" 0

finish_tests "tool-versions"
