#!/usr/bin/env bash
#
# scripts/set-version.sh against a scratch tree (RD-140-22): every copy of the release version
# moves with Cargo.toml (web package, browser manifest, the OpenAPI document's info.version, the
# SDK pins and templates' `min_app_version`), nothing else with a `version` key moves, the
# browser manifest drops a pre-release suffix, and `--check` names a copy that disagrees.
# `cargo update` is a stub here; the lock is cargo's job.
#
#   scripts/tests/set-version.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

TREE="$SCRATCH/tree"
mkdir -p "$TREE/scripts/lib" "$TREE/web" "$TREE/extension" "$TREE/sdk/ci" "$TREE/sdk/templates/demo" \
    "$SCRATCH/bin"
cp "$ROOT/scripts/set-version.sh" "$TREE/scripts/"
cp "$ROOT/scripts/lib/workspace-version.sh" "$TREE/scripts/lib/"
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
# The SDK workflows' release pin, beside a step's own `env:` that must not move.
for workflow in plugin repository; do
    cat > "$TREE/sdk/ci/$workflow.yml" <<'EOF'
env:
  COMPONENT: myhoster
  RDOWNLOADER_VERSION: 1.3.1

jobs:
  plugin:
    env:
      TOOL_VERSION: 1.3.1
EOF
done
# A template's minimum, beside the template's own version that must not move (RD-1250-01).
printf 'api_version = "0.11.0"\nversion = "0.1.0"\n\n[metadata]\nmin_app_version = "1.3.1"\n' \
    > "$TREE/sdk/templates/demo/manifest.toml"
set_version() { run_status "$TREE/scripts/set-version.sh" "$@"; }
template_minimum() { sed -n 's/^min_app_version = "\(.*\)"$/\1/p' "$TREE/sdk/templates/demo/manifest.toml"; }
json_version() { python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["version"])' "$1"; }
sdk_pin() { sed -n 's/^  RDOWNLOADER_VERSION: //p' "$TREE/sdk/ci/$1.yml"; }
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
expect "sdk/ci/plugin.yml pins the release" "1.4.0" "$(sdk_pin plugin)"
expect "sdk/ci/repository.yml pins the release" "1.4.0" "$(sdk_pin repository)"
expect "a template needs the release" "1.4.0" "$(template_minimum)"
expect_true "the template's own version is untouched" 'grep -qF "version = \"0.1.0\"" "$TREE/sdk/templates/demo/manifest.toml"'
expect_true "a step's own version is untouched" 'grep -qF "TOOL_VERSION: 1.3.1" "$TREE/sdk/ci/plugin.yml"'
expect_true "a schema's own version property is untouched" 'grep -qF "\"example\": \"0.1.0\"" "$TREE/web/openapi.json"'
set_version --check
expect_status "--check after the bump" 0
expect_true "the dependency's version is untouched" 'grep -qF "serde = { version = \"1.0.200\" }" "$TREE/Cargo.toml"'
expect_true "package.json keeps its trailing newline" '[[ "$(tail -c1 "$TREE/web/package.json" | od -An -c | tr -d " ")" == "\n" ]]'
expect "the lock is updated through cargo" "update --workspace --offline" "$(tail -1 "$SCRATCH/cargo.calls")"

# The same version again, as a re-started release chain bumps it (RD-1130-01): no copy is
# rewritten, so no mtime moves and web/dist stays current for scripts/web-dist-stale.sh.
copies=(Cargo.toml web/package.json extension/manifest.base.json web/openapi.json sdk/ci/plugin.yml sdk/ci/repository.yml
    sdk/templates/demo/manifest.toml)
for copy in "${copies[@]}"; do touch -d '2001-01-01 00:00' "$TREE/$copy"; done
set_version 1.4.0
expect_status "the version the tree already carries is set again" 0
moved=""
for copy in "${copies[@]}"; do
    [[ "$(stat -c %Y "$TREE/$copy")" == "$(date -d '2001-01-01 00:00' +%s)" ]] || moved+="$copy "
done
expect "and no copy is rewritten" "" "$moved"

set_version 1.5.0-beta.1
expect_status "a pre-release version is set" 0
expect "Cargo.toml carries the suffix" "1.5.0-beta.1" "$("$TREE/scripts/set-version.sh")"
expect "web/package.json carries it" "1.5.0-beta.1" "$(json_version "$TREE/web/package.json")"
expect "web/openapi.json info.version carries it" "1.5.0-beta.1" "$(api_version "$TREE/web/openapi.json")"
expect "the browser manifest drops it" "1.5.0" "$(json_version "$TREE/extension/manifest.base.json")"
expect "the SDK pin names the pre-release tag" "1.5.0-beta.1" "$(sdk_pin plugin)"
expect "a template needs the pre-release" "1.5.0-beta.1" "$(template_minimum)"
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

sed -i 's/^min_app_version = .*/min_app_version = "1.9.0"/' "$TREE/sdk/templates/demo/manifest.toml"
set_version --check
expect_true "a template's lagging minimum is named too" 'grep -qF "sdk/templates/demo/manifest.toml reports 1.9.0" <<< "$output"'

"$TREE/scripts/set-version.sh" 1.5.0-beta.1 > /dev/null 2>&1
sed -i 's/^  RDOWNLOADER_VERSION: .*/  RDOWNLOADER_VERSION: 1.5.2/' "$TREE/sdk/ci/repository.yml"
set_version --check
expect_status "--check: a lagging SDK pin fails" 1
expect_true "and is named" 'grep -qF "sdk/ci/repository.yml reports 1.5.2" <<< "$output"'

# The one list (RD-1270-09): a bump writes exactly what RD_VERSION_FILES names — the list
# lib/verified.sh takes for a version-only change — Cargo.lock aside, which the stub leaves alone.
"$TREE/scripts/set-version.sh" 1.5.0-beta.1 > /dev/null 2>&1
rm -rf "$SCRATCH/before" && cp -a "$TREE" "$SCRATCH/before"
set_version 1.6.0
expect_status "a bump for the list" 0
written="$({ diff -rq "$SCRATCH/before" "$TREE" || true; } | sed -n "s|^Files $SCRATCH/before/\(.*\) and .* differ\$|\1|p" | sort)"
listed="$(cd "$TREE" && source scripts/lib/workspace-version.sh \
    && for entry in "${RD_VERSION_FILES[@]}"; do for path in $entry; do [[ "$path" == Cargo.lock ]] || echo "$path"; done; done | sort)"
expect "the bump writes exactly the listed files" "$listed" "$written"
cp "$TREE/scripts/lib/workspace-version.sh" "$SCRATCH/workspace-version.sh"
sed -i 's|^RD_VERSION_COPIES=(|RD_VERSION_COPIES=(Cargo.toml.orig |' "$TREE/scripts/lib/workspace-version.sh"
touch "$TREE/Cargo.toml.orig"
set_version --check
expect_status "--check: a listed copy without a rule fails" 1
expect_output "and is named" "no rule for the copy Cargo.toml.orig"
set_version 1.7.0
expect_status "and so does the bump" 1
cp "$SCRATCH/workspace-version.sh" "$TREE/scripts/lib/workspace-version.sh"
rm "$TREE/Cargo.toml.orig"
# Python under Git Bash on Windows globs with `\` (smoke windows-2025, 1.27): the rules still match.
sed -i "s|'sdk/templates/\*/manifest.toml'|'sdk\\\\templates\\\\*\\\\manifest.toml'|" "$TREE/scripts/lib/workspace-version.sh"
expect_true "the backslash entry is in place" 'grep -qF "sdk\\templates\\*\\manifest.toml" "$TREE/scripts/lib/workspace-version.sh"'
set_version 1.6.1
expect_status "a path with Windows separators finds its rule" 0
expect "and its copy is written" "1.6.1" "$(template_minimum)"
set_version --check
expect_status "--check reads it too" 0
expect_output "and finds it in agreement" "every copy reports 1.6.1"
cp "$SCRATCH/workspace-version.sh" "$TREE/scripts/lib/workspace-version.sh"
"$TREE/scripts/set-version.sh" 1.6.0 > /dev/null 2>&1

before="$(cat "$TREE/Cargo.toml")"
set_version 1.5
expect_status "not a semantic version: refused" 2
expect "and nothing is written" "$before" "$(cat "$TREE/Cargo.toml")"

finish_tests set-version
