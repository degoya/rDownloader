#!/usr/bin/env bash
#
# scripts/lib/doc-facts.py (RD-140-24) against a small tree built in a temp directory: a workspace
# version, two plugins with a manifest and a library without one, the WIT package line, every
# repository anchor and a two-page wiki. It moves each source in turn — the version, the plugin
# count, the contract — and wants --check to name the stale places and the write to fix exactly
# those; a reworded sentence has to be a refusal, not a pass.
#
# Pure python3 and bash: it runs in a second. check.sh runs it when scripts/lib/ or scripts/tests/
# change, and under --full.
#
#   scripts/tests/doc-facts.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$ROOT/scripts/lib/doc-facts.py"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
TREE="$SCRATCH/repo"
WIKI="$SCRATCH/wiki"

failures=0
passed=0
ok() { echo "ok   $1"; passed=$((passed + 1)); }
fail() { echo "FAIL $1"; failures=$((failures + 1)); }
expect() { if eval "$2"; then ok "$1"; else fail "$1"; fi; }
run() { python3 "$SCRIPT" "$TREE" "$@" > "$SCRATCH/out" 2>&1 && status=0 || status=$?; }
has() { grep -qF -- "$2" "$1"; }

mkdir -p "$TREE/docs" "$TREE/sdk" "$TREE/crates/rd-plugin-api/wit" "$TREE/plugins/a" \
    "$TREE/plugins/b" "$TREE/plugins/common" "$WIKI/plugins"
printf '[workspace]\nmembers = []\n\n[workspace.package]\nedition = "2024"\nversion = "1.3.1"\n\n[workspace.dependencies]\nx = { version = "9.9.9" }\n' \
    > "$TREE/Cargo.toml"
printf 'package rdownloader:plugin@0.9.0;\n' > "$TREE/crates/rd-plugin-api/wit/rdownloader.wit"
touch "$TREE/plugins/a/manifest.toml" "$TREE/plugins/b/manifest.toml" "$TREE/plugins/common/Cargo.toml"
cat > "$TREE/docs/feature-list.md" <<'EOF'
# Features

> As of September 20, 2026 · Source version 1.3.1. This document describes only features.

| Bundled plugins | 2 signed WebAssembly components |

- Versioned WIT interface `rdownloader:plugin@0.9.0` with manifest version 3.
EOF
printf 'Carrying it would make all 2\nsigned components stale. The contract moved to `rdownloader:plugin@0.7.0` once.\n\n- WIT version: the plugin package is `rdownloader:plugin@0.9.0`.\n' \
    > "$TREE/docs/plugins.md"
printf -- '- **Plugins** — WIT interface `rdownloader:plugin@0.9.0`.\n' > "$TREE/docs/development.md"
printf 'Built against the versioned contract `rdownloader:plugin@0.9.0`, for every extension point.\n' > "$TREE/README.md"
printf 'The current package is `rdownloader:plugin@0.9.0` (RD-130-11).\n' > "$TREE/sdk/README.md"
printf '| Bundled plugins | 2 signed WebAssembly components |\n' > "$WIKI/home.md"
printf 'The contract is the WIT package `rdownloader:plugin@0.9.0`.\n' > "$WIKI/plugins/overview.md"
cp -r "$TREE" "$SCRATCH/pristine"

run --check --wiki "$WIKI"
expect "a current tree passes, wiki included" '[[ $status -eq 0 ]] && has "$SCRATCH/out" "doc facts current: version 1.3.1, 2 plugins, rdownloader:plugin@0.9.0 (with the wiki)"'

sed -i 's/^version = "1.3.1"/version = "1.4.0"/' "$TREE/Cargo.toml"
run --check
expect "a moved version is named with its place" '[[ $status -eq 1 ]] && has "$SCRATCH/out" "docs/feature-list.md:3: version is 1.3.1, the source says 1.4.0"'
expect "check writes nothing" 'diff -rq "$SCRATCH/pristine/docs" "$TREE/docs" > /dev/null'
run --date 2026-10-02
expect "the write sets version and date together" '[[ $status -eq 0 ]] && has "$TREE/docs/feature-list.md" "> As of October 2, 2026 · Source version 1.4.0. This document"'
run --date 2026-10-09
expect "a second write changes nothing, the date included" '[[ $status -eq 0 ]] && has "$SCRATCH/out" "0 file(s) changed" && has "$TREE/docs/feature-list.md" "As of October 2, 2026"'
expect "a dependency version is not the workspace version" '! has "$SCRATCH/out" "9.9.9"'

mkdir -p "$TREE/plugins/c" && touch "$TREE/plugins/c/manifest.toml"
run --check --wiki "$WIKI"
expect "a new plugin is named in every place that counts, wrapped lines included" '[[ $status -eq 1 ]] && has "$SCRATCH/out" "docs/feature-list.md:5: plugins is 2, the source says 3" && has "$SCRATCH/out" "docs/plugins.md:1: plugins is 2" && has "$SCRATCH/out" "$WIKI/home.md:1: plugins is 2"'

printf 'package rdownloader:plugin@0.10.0;\n' > "$TREE/crates/rd-plugin-api/wit/rdownloader.wit"
run --wiki "$WIKI"
expect "the write takes the contract everywhere it is stated" '[[ $status -eq 0 ]] && has "$TREE/README.md" "@0.10.0" && has "$TREE/sdk/README.md" "@0.10.0" && has "$TREE/docs/development.md" "@0.10.0" && has "$TREE/docs/feature-list.md" "@0.10.0" && has "$WIKI/plugins/overview.md" "@0.10.0"'
expect "a historical mention is not an anchor" 'has "$TREE/docs/plugins.md" "moved to \`rdownloader:plugin@0.7.0\` once" && has "$TREE/docs/plugins.md" "package is \`rdownloader:plugin@0.10.0\`"'
expect "the count moved with it, the wrapped line intact" 'has "$TREE/docs/plugins.md" "all 3" && has "$TREE/docs/feature-list.md" "| Bundled plugins | 3 signed" && has "$WIKI/home.md" "| 3 signed"'
run --check --wiki "$WIKI"
expect "check after the write exits 0" '[[ $status -eq 0 ]]'

sed -i 's/the versioned contract/the contract/' "$TREE/README.md"
run --check
expect "a reworded sentence is a refusal naming the file" '[[ $status -eq 2 ]] && has "$SCRATCH/out" "README.md: no match for the anchor"'

echo
echo "$passed passed, $failures failed"
[[ "$failures" -eq 0 ]]
