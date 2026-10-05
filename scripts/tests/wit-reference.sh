#!/usr/bin/env bash
#
# scripts/lib/wit-reference.py and scripts/check-sdk-templates.sh (RD-160-04) against a small tree
# built in a temp directory: a two-world contract with a record, a variant, an enum and doc
# comments, a wiki page with the markers, and one template per world. It wants the reference to
# carry every item and its docs, the write to land between the markers and nothing else, --check
# to name a stale page, a construct the reader does not know to be a refusal, a job id to stay out
# of the page or be a refusal; and the template
# check to name a world without a template, a template without a world, and a drifted contract.
#
# Pure python3 and bash: it runs in a second. check.sh runs it when scripts/ changes, and under
# --full.
#
#   scripts/tests/wit-reference.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$ROOT/scripts/lib/wit-reference.py"
TEMPLATES_CHECK="$ROOT/scripts/check-sdk-templates.sh"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
TREE="$SCRATCH/repo"
WIKI="$SCRATCH/wiki"
WIT="$TREE/crates/rd-plugin-api/wit/rdownloader.wit"

# shellcheck source=scripts/tests/lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
run() { run_status python3 "$SCRIPT" "$TREE" "$@"; }
templates() { run_status bash "$TEMPLATES_CHECK" "$TREE"; }
said() { grep -qF -- "$1" <<< "$output"; }
has() { grep -qF -- "$2" "$1"; }

mkdir -p "$(dirname "$WIT")" "$WIKI/plugins"
cat > "$WIT" <<'EOF'
package rdownloader:plugin@0.9.0;

interface types {
  /// Why a call failed.
  record failure {
    /// English, redaction-safe | text.
    message: string,
    code: option<string>,
  }

  enum link-status {
    online,
    /// Held ready right now.
    cached,
  }
}

/// Always granted (RD-090-12).
interface host {
  use types.{failure};

  variant answer {
    token(string),
    nothing,
  }

  /// Waits out a countdown.
  ///
  /// The host owns the clock
  /// (RD-100-01, ADR 0003).
  wait: func(seconds: u32) -> result<_, failure>;
  log: func(level: string, message: string);
}

/// The plain one.
world alpha-plugin {
  import host;
  export types;
}

world beta-plugin {
  import host;
  export host;
}
EOF
cat > "$WIKI/plugins/plugin-reference.md" <<'EOF'
# Plugin reference

Hand-written text before.

<!-- BEGIN wit-reference -->
stale
<!-- END wit-reference -->

Hand-written text after.
EOF
# What check-sdk-templates.sh holds the templates to besides their worlds (RD-1101-17): the
# workspace's version and wit-bindgen, the API versions the core accepts and the SDK pins.
mkdir -p "$TREE/crates/rd-plugin-host/src" "$TREE/sdk/ci"
printf '[workspace.package]\nversion = "1.10.0"\n\n[workspace.dependencies]\nwit-bindgen = "0.62"\n' \
    > "$TREE/Cargo.toml"
printf 'pub const SUPPORTED_API_VERSIONS: &[&str] = &["0.10.0"];\n' \
    > "$TREE/crates/rd-plugin-host/src/manifest.rs"
for workflow in plugin repository; do
    printf 'env:\n  RDOWNLOADER_VERSION: 1.10.0\n' > "$TREE/sdk/ci/$workflow.yml"
done
for world in alpha beta; do
    mkdir -p "$TREE/sdk/templates/$world/wit" "$TREE/sdk/templates/$world/src"
    cp "$WIT" "$TREE/sdk/templates/$world/wit/rdownloader.wit"
    printf '[dependencies]\nwit-bindgen = "0.62"\n' > "$TREE/sdk/templates/$world/Cargo.toml"
    printf 'wit_bindgen::generate!({\n    path: "wit",\n    world: "%s-plugin",\n});\n' "$world" \
        > "$TREE/sdk/templates/$world/src/guest.rs"
    printf 'api_version = "0.10.0"\nplugin_type = "%s"\n\n[metadata]\nmin_app_version = "1.9.0"\n' "$world" \
        > "$TREE/sdk/templates/$world/manifest.toml"
    printf '# {{PLUGIN_NAME}}\n' > "$TREE/sdk/templates/$world/README.md"
    printf '#[test]\nfn works() {}\n' > "$TREE/sdk/templates/$world/src/lib.rs"
done
cp "$WIKI/plugins/plugin-reference.md" "$SCRATCH/page.orig"

run --check
expect_true "the contract is read completely" '[[ $status -eq 0 ]] && said "read completely (2 worlds, 2 interfaces)"'

run --print
expect_true "every world is listed with its imports, exports and scaffold" 'said "| \`alpha-plugin\` | \`host\` | \`types\` | \`--type alpha\` |"'
expect_true "a world keeps its doc comment" 'said "- \`alpha-plugin\`: The plain one."'
expect_true "a function carries its signature and every paragraph of its docs" 'said "- \`wait: func(seconds: u32) -> result<_, failure>\`" && said "  The host owns the clock." && said "- \`log: func(level: string, message: string)\`"'
expect_true "a record lists its fields, a pipe in a doc escaped" 'said "| \`message\` | \`string\` | English, redaction-safe \\| text. |"'
expect_true "variants and enums list their cases" 'said "| \`token\` | \`string\` |  |" && said "| \`cached\` | Held ready right now. |"'
expect_true "a use is named" 'said "Uses \`failure\` from \`types\`."'
expect_true "job ids and ADRs in parentheses stay out of the wiki, also on a line of their own" 'said "Always granted." && ! said "RD-" && ! said "ADR 0003"'

run --check --wiki "$WIKI"
expect_true "a stale page is exit 1 and names the command" '[[ $status -eq 1 ]] && said "run scripts/wit-reference.sh --wiki"'
expect_true "check writes nothing" 'cmp -s "$SCRATCH/page.orig" "$WIKI/plugins/plugin-reference.md"'
run --wiki "$WIKI"
expect_true "the write replaces the part between the markers and nothing else" '[[ $status -eq 0 ]] && ! has "$WIKI/plugins/plugin-reference.md" "stale" && has "$WIKI/plugins/plugin-reference.md" "Hand-written text before." && has "$WIKI/plugins/plugin-reference.md" "Hand-written text after." && has "$WIKI/plugins/plugin-reference.md" "### Contract reference: \`rdownloader:plugin@0.9.0\`"'
run --check --wiki "$WIKI"
expect_true "check after the write exits 0" '[[ $status -eq 0 ]] && said "is current"'
cp "$WIKI/plugins/plugin-reference.md" "$SCRATCH/page.written"
run --wiki "$WIKI"
expect_true "a second write changes nothing" 'cmp -s "$SCRATCH/page.written" "$WIKI/plugins/plugin-reference.md"'

sed -i 's/^\/\/\/ Always granted (RD-090-12)\./\/\/\/ Always granted, see RD-090-12./' "$WIT"
run --print
expect_true "a job id the sentence needs is a refusal naming it" '[[ $status -eq 2 ]] && said "a job id outside a parenthesis of its own: Always granted, see RD-090-12."'
sed -i 's/^\/\/\/ Always granted, see RD-090-12\./\/\/\/ Always granted (RD-090-12)./' "$WIT"

sed -i 's/<!-- END wit-reference -->//' "$WIKI/plugins/plugin-reference.md"
run --check --wiki "$WIKI"
expect_true "a page without both markers is a refusal" '[[ $status -eq 2 ]] && said "needs exactly one"'

templates
expect_true "two worlds, two complete templates" '[[ $status -eq 0 ]] && said "2 worlds, 2 templates"'

sed -i 's/^  log: func/  resource thing;\n  log: func/' "$WIT"
run --check
expect_true "a construct the reader does not know is a refusal naming the line" '[[ $status -eq 2 ]] && said "not understood in interface \`host\`: resource thing;"'

templates
expect_true "a template whose contract drifted is named" '[[ $status -eq 1 ]] && said "sdk/templates/alpha/wit/rdownloader.wit is not the current contract"'

printf '\nworld gamma-plugin {\n  import host;\n  export host;\n}\n' >> "$WIT"
for world in alpha beta; do cp "$WIT" "$TREE/sdk/templates/$world/wit/rdownloader.wit"; done
mkdir -p "$TREE/sdk/templates/delta"
templates
expect_true "a world without a template and a template without a world are both named" '[[ $status -eq 1 ]] && said "world gamma-plugin has no template" && said "sdk/templates/delta belongs to no world"'

finish_tests wit-reference
