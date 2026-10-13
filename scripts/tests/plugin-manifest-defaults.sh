#!/usr/bin/env bash
#
# scripts/plugin-manifest-defaults.sh and completed_manifest (scripts/lib/plugin-drift.sh) on a
# scratch plugin (RD-1280-18): the conversion removes a shared field and a default limit, takes a
# comment with the line it explained and leaves one that introduces the next field, drops a
# [limits] table left empty, changes nothing on a second run, and --check names a stated default;
# completion writes the shared fields back where the Rust side does.
#
#   scripts/tests/plugin-manifest-defaults.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

TREE="$SCRATCH/tree"
mkdir -p "$TREE/scripts/lib" "$TREE/crates/rd-plugin-host/src" "$TREE/plugins/demo" "$TREE/plugins/quiet"
cp "$ROOT/scripts/plugin-manifest-defaults.sh" "$TREE/scripts/"
cp "$ROOT/scripts/lib/plugin-manifest-defaults.py" "$TREE/scripts/lib/"
cp "$ROOT/crates/rd-plugin-host/src/lib.rs" "$TREE/crates/rd-plugin-host/src/"
cat > "$TREE/plugins/metadata.toml" <<'EOF'
# Shared.
manifest_version = 3
api_version = "0.11.0"
key_id = "demo-v1"
public_key = "AAAA"

[metadata]
author = "Demo"
min_app_version = "1.9.0"
EOF
cat > "$TREE/plugins/demo/manifest.toml" <<'EOF'
manifest_version = 3
plugin_type = "resolver"
api_version = "0.11.0"
name = "Demo"
version = "1.0.0"
key_id = "demo-v1"
public_key = "AAAA"

[metadata]
description = "A demo."
author = "Demo"
# Needs the newer core.
min_app_version = "1.27.0"

[limits]
memory_bytes = 33554432
# These bound a runaway.
fuel = 2000000000
timeout_milliseconds = 30000
# Room for a whole tree.
max_response_bytes = 8388608
EOF
cat > "$TREE/plugins/quiet/manifest.toml" <<'EOF'
manifest_version = 3
name = "Quiet"

[metadata]
description = "Nothing of its own."
author = "Demo"
min_app_version = "1.9.0"

[limits]
memory_bytes = 67108864
# Waiting only.
wait_budget_milliseconds = 600000
EOF

run_status "$TREE/scripts/plugin-manifest-defaults.sh" --check
expect_status "--check names the manifests that state a default" 1
expect_output "and names the demo" "states a default: plugins/demo/manifest.toml"

run_status "$TREE/scripts/plugin-manifest-defaults.sh"
expect_status "the conversion runs" 0
expect "the demo keeps what differs, and the comment that introduces a field" 'plugin_type = "resolver"
name = "Demo"
version = "1.0.0"

[metadata]
description = "A demo."
# Needs the newer core.
min_app_version = "1.27.0"

[limits]
memory_bytes = 33554432
# These bound a runaway.
timeout_milliseconds = 30000' "$(cat "$TREE/plugins/demo/manifest.toml")"
expect "a table left without a field goes" 'name = "Quiet"

[metadata]
description = "Nothing of its own."' "$(cat "$TREE/plugins/quiet/manifest.toml")"

run_status "$TREE/scripts/plugin-manifest-defaults.sh"
expect "a second run changes nothing" "" "$output"
run_status "$TREE/scripts/plugin-manifest-defaults.sh" --check
expect_status "--check is clean after the conversion" 0

# completed_manifest, run where build-plugins.sh runs it: the checkout root.
completed="$(cd "$TREE" && bash -c "source '$ROOT/scripts/lib/plugin-drift.sh'; completed_manifest demo")"
# shellcheck disable=SC2016  # the backticks are the marker's own text
expect "completion writes the shared fields back, the manifest's own value kept" '# Completed from the shared plugin fields by `rd-pack plugin package` (RD-1280-18).
manifest_version = 3
api_version = "0.11.0"
key_id = "demo-v1"
public_key = "AAAA"
plugin_type = "resolver"
name = "Demo"
version = "1.0.0"

[metadata]
author = "Demo"
description = "A demo."
# Needs the newer core.
min_app_version = "1.27.0"

[limits]
memory_bytes = 33554432
# These bound a runaway.
timeout_milliseconds = 30000' "$completed"

rm "$TREE/plugins/metadata.toml"
completed="$(cd "$TREE" && bash -c "source '$ROOT/scripts/lib/plugin-drift.sh'; completed_manifest quiet")"
expect "without the shared file a manifest stays as it is" "$(cat "$TREE/plugins/quiet/manifest.toml")" "$completed"

finish_tests plugin-manifest-defaults
