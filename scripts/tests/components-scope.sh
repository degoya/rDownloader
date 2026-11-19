#!/usr/bin/env bash
#
# The scope of check.sh's version gate (scripts/lib/components.sh) against a scratch tree with a
# stand-in build-plugins.sh: on the branch level a change to Cargo.lock alone asks about every
# plugin (RD-1430-11), since every component's stamp includes the registry packages it pins; a
# change to one plugin asks about that one, a change to neither asks about none, and a plugin the
# lock-file change left differing from its signed package stops the run, named.
#
#   scripts/tests/components-scope.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

# The stand-in logs its calls; --list-unbumped prints what $SCRATCH/drift lists.
TREE="$SCRATCH/tree"
mkdir -p "$TREE/scripts" "$TREE/plugins/alpha" "$TREE/plugins/bravo"
touch "$TREE/plugins/alpha/manifest.toml" "$TREE/plugins/bravo/manifest.toml"
cat > "$TREE/scripts/build-plugins.sh" << STUB
#!/usr/bin/env bash
echo "build-plugins.sh \$*" >> "$SCRATCH/calls"
if [[ "\$1" == --list-unbumped ]]; then cat "$SCRATCH/drift" 2> /dev/null || true; fi
STUB
chmod +x "$TREE/scripts/build-plugins.sh"

# What check.sh defines before it sources lib/components.sh.
# shellcheck disable=SC2034  # read by rd_component_gates
TARGET_DIR="$SCRATCH/target"
step() { echo "==> $*"; }
touches() { [[ -n "$changed" ]] && grep -qE "$1" <<< "$changed"; }
rd_plugin_linked_crates() { echo "crates/rd-plugin-api/"; }
# shellcheck source=../lib/components.sh
source "$ROOT/scripts/lib/components.sh"

# The gate on the branch level for the changed paths in "$@", in the scratch tree.
gates() {
    # shellcheck disable=SC2034  # read by rd_component_gates
    full=0
    changed="$(printf '%s\n' "$@")"
    cd "$TREE"
    rd_component_gates
}

rm -f "$SCRATCH/calls"
run_status gates Cargo.lock
expect_status "a change to Cargo.lock alone" 0
expect_output "asks about every plugin" "checked every plugin, since the change touches Cargo.lock"
expect "with no plugin named, which is all of them" "build-plugins.sh --list-unbumped" \
    "$(grep -- '--list-unbumped' "$SCRATCH/calls")"

echo "alpha 0.1.0 component dist/plugins/alpha-0.1.0.rdplugin" > "$SCRATCH/drift"
run_status gates Cargo.lock
expect_status "a component the lock file changed under its signed version stops the run" 1
expect_output "naming it" "alpha 0.1.0 — component differs from dist/plugins/alpha-0.1.0.rdplugin"
rm -f "$SCRATCH/drift"

rm -f "$SCRATCH/calls"
run_status gates plugins/alpha/src/lib.rs
expect_status "a change to one plugin" 0
expect_output "asks about the plugins it touches" "checked the plugins this change touches"
expect "that one by name" "build-plugins.sh --list-unbumped alpha" "$(grep -- '--list-unbumped' "$SCRATCH/calls")"

rm -f "$SCRATCH/calls"
run_status gates README.md
expect_status "a change to neither" 0
expect_output "asks about none" "no plugin in the change set"
expect_true "and calls no --list-unbumped" '! grep -q -- "--list-unbumped" "$SCRATCH/calls"'

finish_tests components-scope
