#!/usr/bin/env bash
#
# scripts/check-sdk-templates.sh against a fixture tree with one world (audit K2, K4): a complete
# template passes; a guest generating another world, a template on another wit-bindgen than the
# workspace, an api_version the core does not accept, a min_app_version that is not the first
# release of its api_version, a contract version without a row, an SDK workflow pinned to another
# release than the workspace and a pre-release pin older than min_app_version each fail, named.
# Last, this checkout's own templates, which have to pass as they are.
#
# Pure bash: it runs in well under a second.
#
#   scripts/tests/check-sdk-templates.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$ROOT/scripts/check-sdk-templates.sh"
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

TREE="$SCRATCH/tree"
TEMPLATE="$TREE/sdk/templates/demo"
fixture() {
    rm -rf "$TREE"
    mkdir -p "$TREE/crates/rd-plugin-api/wit" "$TREE/crates/rd-plugin-host/src" "$TREE/sdk/ci" \
        "$TEMPLATE/wit" "$TEMPLATE/src"
    cat > "$TREE/Cargo.toml" <<'EOF'
[workspace.package]
version = "1.10.0"

[workspace.dependencies]
wit-bindgen = "0.62"
EOF
    printf 'pub const SUPPORTED_API_VERSIONS: &[&str] = &["0.10.0"];\n' \
        > "$TREE/crates/rd-plugin-host/src/manifest.rs"
    printf 'package rdownloader:plugin@0.10.0;\n\nworld demo-plugin {\n}\n' \
        > "$TREE/crates/rd-plugin-api/wit/rdownloader.wit"
    cp "$TREE/crates/rd-plugin-api/wit/rdownloader.wit" "$TEMPLATE/wit/"
    printf '[dependencies]\nwit-bindgen = "0.62"\n' > "$TEMPLATE/Cargo.toml"
    printf 'wit_bindgen::generate!({\n    path: "wit",\n    world: "demo-plugin",\n});\n' \
        > "$TEMPLATE/src/guest.rs"
    printf 'api_version = "0.10.0"\nplugin_type = "demo"\n\n[metadata]\nmin_app_version = "1.9.0"\n' \
        > "$TEMPLATE/manifest.toml"
    echo "# Demo" > "$TEMPLATE/README.md"
    printf '# Changes\n\n## 0.1.0\n\nThe first release.\n' > "$TEMPLATE/CHANGES.md"
    printf '#[test]\nfn works() {}\n' > "$TEMPLATE/src/lib.rs"
    for workflow in plugin repository; do
        printf 'env:\n  RDOWNLOADER_VERSION: 1.10.0\n' > "$TREE/sdk/ci/$workflow.yml"
    done
}
check() { run_status "$SCRIPT" "$TREE"; }

fixture
check
expect_status "a complete template passes" 0

fixture
sed -i 's/demo-plugin/other-plugin/' "$TEMPLATE/src/guest.rs"
check
expect_status "a guest that generates another world: refused" 1
expect_output "and named" "src/guest.rs does not generate world demo-plugin"

fixture
rm "$TEMPLATE/CHANGES.md"
check
expect_status "a template without its release notes: refused" 1
expect_output "naming the file" "sdk/templates/demo has no CHANGES.md"

fixture
sed -i 's/^wit-bindgen = .*/wit-bindgen = "0.61"/' "$TEMPLATE/Cargo.toml"
check
expect_status "another wit-bindgen than the workspace: refused" 1
expect_output "and named" "asks for wit-bindgen 0.61, the workspace for 0.62"

fixture
sed -i 's/^api_version = .*/api_version = "0.9.0"/' "$TEMPLATE/manifest.toml"
check
expect_status "an api_version the core does not accept: refused" 1
expect_output "and named" "declares api_version 0.9.0, this core accepts 0.10.0"

fixture
sed -i 's/^min_app_version = .*/min_app_version = "0.9.0"/' "$TEMPLATE/manifest.toml"
check
expect_status "a min_app_version below the api_version's first release: refused" 1
expect_output "and named" "says min_app_version 0.9.0, api_version 0.10.0 needs 1.9.0"

fixture
sed -i 's/&\["0.10.0"\]/\&["0.10.0", "0.11.0"]/' "$TREE/crates/rd-plugin-host/src/manifest.rs"
sed -i 's/^api_version = .*/api_version = "0.11.0"/' "$TEMPLATE/manifest.toml"
check
expect_status "a contract version without a row: refused" 1
expect_output "and named" "api_version 0.11.0 has no row in api_first_release"

fixture
sed -i 's/^  RDOWNLOADER_VERSION: .*/  RDOWNLOADER_VERSION: 1.5.2/' "$TREE/sdk/ci/plugin.yml"
check
expect_status "an SDK workflow pinned to another release: refused" 1
expect_output "and named" "sdk/ci/plugin.yml pins rDownloader 1.5.2, the workspace is 1.10.0"

fixture
sed -i 's/^version = .*/version = "1.9.0-beta.1"/' "$TREE/Cargo.toml"
sed -i 's/^  RDOWNLOADER_VERSION: .*/  RDOWNLOADER_VERSION: 1.9.0-beta.1/' "$TREE"/sdk/ci/*.yml
check
expect_status "a pre-release pin older than min_app_version: refused" 1
expect_output "and named" "pins rDownloader 1.9.0-beta.1, older than the templates' min_app_version 1.9.0"

run_status "$SCRIPT"
expect_status "this checkout's templates pass" 0

finish_tests check-sdk-templates
