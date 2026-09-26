#!/usr/bin/env bash
#
# scripts/set-version.sh against a scratch tree (RD-140-22): the three files that carry the
# release version move together, nothing else with a `version` key moves, and the browser
# manifest drops a pre-release suffix. `cargo update` is a stub here; the lock is cargo's job.
#
#   scripts/tests/set-version.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

TREE="$SCRATCH/tree"
mkdir -p "$TREE/scripts" "$TREE/web" "$TREE/extension" "$SCRATCH/bin"
cp "$ROOT/scripts/set-version.sh" "$TREE/scripts/"
printf '#!/bin/sh\necho "$@" >> "%s/cargo.calls"\n' "$SCRATCH" > "$SCRATCH/bin/cargo"
chmod +x "$SCRATCH/bin/cargo"
export PATH="$SCRATCH/bin:$PATH"

cat > "$TREE/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/*"]

[workspace.package]
edition = "2024"
version = "1.3.1"
license = "MIT"

[workspace.dependencies]
serde = { version = "1.0.200" }

[profile.release]
lto = "thin"
EOF
printf '{\n  "name": "web",\n  "version": "1.3.1",\n  "private": true\n}\n' > "$TREE/web/package.json"
printf '{\n  "name": "ext",\n  "version": "1.3.1"\n}\n' > "$TREE/extension/manifest.base.json"
set_version() { run_status "$TREE/scripts/set-version.sh" "$@"; }
json_version() { python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["version"])' "$1"; }

set_version
expect_status "without an argument it reads" 0
expect "the workspace version, not a dependency's" "1.3.1" "$output"

set_version 1.4.0
expect_status "a release version is set" 0
expect "Cargo.toml" "1.4.0" "$(sed -n 's/^version = "\(.*\)"/\1/p' "$TREE/Cargo.toml")"
expect "web/package.json" "1.4.0" "$(json_version "$TREE/web/package.json")"
expect "extension/manifest.base.json" "1.4.0" "$(json_version "$TREE/extension/manifest.base.json")"
expect_true "the dependency's version is untouched" 'grep -qF "serde = { version = \"1.0.200\" }" "$TREE/Cargo.toml"'
expect_true "package.json keeps its trailing newline" '[[ "$(tail -c1 "$TREE/web/package.json" | od -An -c | tr -d " ")" == "\n" ]]'
expect "the lock is updated through cargo" "update --workspace --offline" "$(tail -1 "$SCRATCH/cargo.calls")"

set_version 1.5.0-rc.1
expect_status "a pre-release version is set" 0
expect "Cargo.toml carries the suffix" "1.5.0-rc.1" "$("$TREE/scripts/set-version.sh")"
expect "the browser manifest drops it" "1.5.0" "$(json_version "$TREE/extension/manifest.base.json")"

before="$(cat "$TREE/Cargo.toml")"
set_version 1.5
expect_status "not a semantic version: refused" 2
expect "and nothing is written" "$before" "$(cat "$TREE/Cargo.toml")"

finish_tests set-version
