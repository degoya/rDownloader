#!/usr/bin/env bash
#
# The files outside crates/ that Rust tests read (RD-191-09): rd_scope_input_packages and the
# deferral refusal in scripts/lib/scope.sh, and scripts/lib/rust-test-inputs.py, which holds the
# table against the sources, on a scratch repository. Last, the real table against this tree.
#
# Pure bash, python3 and git, no cargo. check.sh runs it when scripts/ change, and under --full.
#
#   scripts/tests/rust-test-inputs.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"
# shellcheck source=../lib/scope.sh
source "$ROOT/scripts/lib/scope.sh"
GUARD="$ROOT/scripts/lib/rust-test-inputs.py"

cd "$SCRATCH"
mkdir -p scripts/lib crates/rd-x/src crates/rd-x/tests crates/rd-y/src web/src/locales/en \
    web/src/locales/de dist/plugins
cat > scripts/lib/rust-test-inputs.map <<'EOF'
# a comment
^web/src/locales/[^/]+/logs\.json$     rd-x
^web/src/locales/en/server\.json$      rd-x rd-y
^dist/                                 -
EOF
printf '[package]\nname = "rd-x"\n' > crates/rd-x/Cargo.toml
printf '[package]\nname = "rd-y"\n' > crates/rd-y/Cargo.toml
echo '{}' | tee web/src/locales/en/logs.json web/src/locales/de/logs.json \
    web/src/locales/en/server.json web/src/locales/en/ui.json > /dev/null
touch dist/plugins/.keep crates/rd-y/src/data.json

packages() { printf '%s\n' "$@" | rd_scope_input_packages | paste -sd' ' -; }

# --- which crates a change reaches -------------------------------------------------------------

expect "a catalogue a test reads names its crate" "rd-x" "$(packages web/src/locales/de/logs.json)"
expect "two rows' crates, once each, sorted" "rd-x rd-y" \
    "$(packages web/src/locales/en/logs.json web/src/locales/en/server.json)"
expect "a catalogue no test reads names none" "" "$(packages web/src/locales/en/ui.json)"
expect "a row of - names none" "" "$(packages dist/plugins/x.rdplug)"
expect "the regex is anchored as written" "" "$(packages old/web/src/locales/en/logs.json)"
expect "no table, no crates" "" "$(RD_RUST_INPUTS_MAP=missing.map packages web/src/locales/en/logs.json)"

# --- deferral ----------------------------------------------------------------------------------

expect "a catalogue a Rust test reads is not deferrable" "no" \
    "$(rd_defer_class HEAD web/src/locales/en/logs.json)"
expect "another catalogue still is" "locales" "$(rd_defer_class HEAD web/src/locales/en/ui.json)"
expect "documentation still is" "docs" "$(rd_defer_class HEAD docs/x.md)"

# --- the table against the sources -------------------------------------------------------------

cat > crates/rd-x/src/lib.rs <<'EOF'
const LOGS: &str = include_str!("../../../web/src/locales/en/logs.json");
const OWN: &str = include_str!("../../rd-y/src/data.json");
fn root() { let _ = root.join("web/src/locales"); let _ = base.join("downloads/../web"); }
fn packages() { let _ = temp.join("dist/plugins"); let _ = temp.join("scripts/inbox"); }
EOF
git init -q
git -c user.name=t -c user.email=t@t add -A
git -c user.name=t -c user.email=t@t commit -qm fixture

run_status python3 "$GUARD" .
expect_status "every read file has its row: silent, exit 0" 0
expect "and prints nothing" "" "$output"

cat > crates/rd-y/src/lib.rs <<'EOF'
fn core() { let _ = manifest.join("../../web/src/locales/en/ui.json"); }
fn server() { let _ = manifest.join("../../web/src/locales/en/server.json"); }
EOF
cat > crates/rd-x/tests/notes.rs <<'EOF'
fn languages() { let _ = repository().join("web/src/locales"); }
EOF
git add -A
run_status python3 "$GUARD" .
expect_status "a read file without a row naming the crate fails" 1
expect_output "it names source, path and crate" \
    "crates/rd-y/src/lib.rs reads web/src/locales/en/ui.json (\"../../web/src/locales/en/ui.json\"): no row of scripts/lib/rust-test-inputs.map names rd-y for it"
expect_true "a row naming the crate covers the file" '! grep -q "server.json" <<< "$output"'
expect_true "a directory is covered by a row matching a file under it" '! grep -q "tests/notes.rs" <<< "$output"'
expect_true "another crate's file, a temp path and an untracked path need none" \
    '! grep -qE "rd-y/src/data|dist/plugins|scripts/inbox|downloads" <<< "$output"'

# --- this tree -----------------------------------------------------------------------------------

cd "$ROOT"
run_status python3 "$GUARD" .
expect_status "this tree's Rust sources and table agree" 0

finish_tests "rust-test-inputs"
