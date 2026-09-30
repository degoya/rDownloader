#!/usr/bin/env bash
#
# scripts/set-version.sh against a scratch tree (RD-140-22): every copy of the release version
# moves with Cargo.toml (web package, browser manifest, the OpenAPI document's info.version),
# nothing else with a `version` key moves, the browser manifest drops a pre-release suffix, and
# `--check` names a copy that disagrees. `cargo update` is a stub here; the lock is cargo's job.
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
# The generated document's shape: info.version is the last key of "info", a schema below has a
# `version` property of its own that must not move.
cat > "$TREE/web/openapi.json" <<'EOF'
{
  "openapi": "3.1.0",
  "info": {
    "title": "rd-api",
    "license": {
      "name": "GPL-3.0-or-later"
    },
    "version": "1.3.1"
  },
  "paths": {},
  "components": {
    "schemas": {
      "Plugin": {
        "properties": {
          "version": {
            "type": "string",
            "example": "0.1.0"
          }
        }
      }
    }
  }
}
EOF
set_version() { run_status "$TREE/scripts/set-version.sh" "$@"; }
json_version() { python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["version"])' "$1"; }
api_version() { python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["info"]["version"])' "$1"; }

set_version --check
expect_status "--check: every copy agrees" 0

set_version
expect_status "without an argument it reads" 0
expect "the workspace version, not a dependency's" "1.3.1" "$output"

set_version 1.4.0
expect_status "a release version is set" 0
expect "Cargo.toml" "1.4.0" "$(sed -n 's/^version = "\(.*\)"/\1/p' "$TREE/Cargo.toml")"
expect "web/package.json" "1.4.0" "$(json_version "$TREE/web/package.json")"
expect "extension/manifest.base.json" "1.4.0" "$(json_version "$TREE/extension/manifest.base.json")"
expect "web/openapi.json info.version" "1.4.0" "$(api_version "$TREE/web/openapi.json")"
expect_true "a schema's own version property is untouched" 'grep -qF "\"example\": \"0.1.0\"" "$TREE/web/openapi.json"'
set_version --check
expect_status "--check after the bump" 0
expect_true "the dependency's version is untouched" 'grep -qF "serde = { version = \"1.0.200\" }" "$TREE/Cargo.toml"'
expect_true "package.json keeps its trailing newline" '[[ "$(tail -c1 "$TREE/web/package.json" | od -An -c | tr -d " ")" == "\n" ]]'
expect "the lock is updated through cargo" "update --workspace --offline" "$(tail -1 "$SCRATCH/cargo.calls")"

set_version 1.5.0-beta.1
expect_status "a pre-release version is set" 0
expect "Cargo.toml carries the suffix" "1.5.0-beta.1" "$("$TREE/scripts/set-version.sh")"
expect "web/package.json carries it" "1.5.0-beta.1" "$(json_version "$TREE/web/package.json")"
expect "web/openapi.json info.version carries it" "1.5.0-beta.1" "$(api_version "$TREE/web/openapi.json")"
expect "the browser manifest drops it" "1.5.0" "$(json_version "$TREE/extension/manifest.base.json")"
set_version --check
expect_status "--check on a beta: the bare manifest version agrees" 0

python3 - "$TREE/web/package.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1])); data['version'] = '0.0.1'
open(sys.argv[1], 'w').write(json.dumps(data, indent=2) + '\n')
PY
set_version --check
expect_status "--check: a lagging copy fails" 1
expect_true "and is named" 'grep -qF "web/package.json reports 0.0.1" <<< "$output"'

before="$(cat "$TREE/Cargo.toml")"
set_version 1.5
expect_status "not a semantic version: refused" 2
expect "and nothing is written" "$before" "$(cat "$TREE/Cargo.toml")"

finish_tests set-version
