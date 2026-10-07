#!/usr/bin/env bash
#
# check.sh --full per crate and per kind (RD-1150-06, lib/check-reuse-crates.sh and
# lib/crate-graph.sh) against a toy workspace with a --full green on record: what a run builds on,
# which members it tests, the rd-api binaries, crash runs and sqlx among them, and when the whole
# half runs instead. The cases of the job: a test change in a leaf crate tests that crate only; a
# change in a crate with dependants tests the whole reverse hull; Cargo.lock runs everything;
# translation catalogues alone run Vitest alone; nothing changed ends at once; --again runs all.
#
# Pure git and bash, no cargo: the decisions are run, not the checks.
#
#   scripts/tests/check-reuse-crates.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export CARGO_TARGET_DIR="$SCRATCH/target"
export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid

# --- the toy workspace ------------------------------------------------------------------------------
# base <- leaf, base <- mid <- rd-api, mid <-(dev) top, base <- plugins/guest; mid builds on sqlx;
# the crash list runs mid and one rd-api suite. Its own copy of scripts/lib/, so the lists beside
# the libraries are the toy's.
repo="$SCRATCH/repo"
git init -q -b development "$repo"
cd "$repo"
mkdir -p scripts/lib
cp "$ROOT/scripts/lib/"*.sh "$repo/scripts/lib/"
printf 'mid\nrd-api      --test one alpha\n' > scripts/lib/crash-matrix.list
printf '^web/src/locales/[^/]+/logs\\.json$   leaf\n' > scripts/lib/rust-test-inputs.map
cat > Cargo.toml <<'EOF'
[workspace]
members = ["crates/*", "plugins/guest"]

[workspace.dependencies]
sqlx = "0.9"
EOF
echo '# lock' > Cargo.lock
manifest() {
    local dir="$1" name="$2"
    shift 2
    mkdir -p "$dir/src"
    { printf '[package]\nname = "%s"\nversion = "0.1.0"\n\n[dependencies]\n' "$name"; printf '%s\n' "$@"; } > "$dir/Cargo.toml"
    echo "pub fn f() {}" > "$dir/src/lib.rs"
}
manifest crates/base base
manifest crates/leaf leaf 'base = { path = "../base" }'
manifest crates/mid mid 'base = { path = "../base" }' 'sqlx.workspace = true'
manifest crates/top top '' '[dev-dependencies]' 'mid = { path = "../mid" }'
manifest crates/rd-api rd-api 'mid = { path = "../mid" }'
manifest crates/rd-core rd-core
manifest plugins/guest guest 'base = { path = "../../crates/base" }'
# Test code: a test module behind #[cfg(test)], one that is not, an integration test, a fixture
# mid's library reads and a comment in top names.
printf '#[cfg(test)]\nmod parse_tests;\n' >> crates/leaf/src/lib.rs
echo "#[test] fn t() {}" > crates/leaf/src/parse_tests.rs
printf '/// helpers for the dependants\npub mod helpers_tests;\n' >> crates/base/src/lib.rs
printf '\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {\n        assert_eq!(1, 1);\n    }\n}\n' >> crates/base/src/lib.rs
echo "pub fn helper() {}" > crates/base/src/helpers_tests.rs
mkdir -p crates/leaf/tests/fixtures
echo "#[test] fn it() {}" > crates/leaf/tests/it.rs
echo "<html>" > crates/leaf/tests/fixtures/page.html
mkdir -p crates/leaf/tests/data
echo "<html>" > crates/leaf/tests/data/only.html
echo 'pub const PAGE: &str = include_str!("../../leaf/tests/fixtures/page.html");' >> crates/mid/src/lib.rs
echo '// leaf/tests/data/only.html is the other one' >> crates/top/src/lib.rs
for binary in one two; do mkdir -p "crates/rd-api/tests/$binary"; done
printf 'mod common;\nmod alpha;\n' > crates/rd-api/tests/one/main.rs
printf 'mod common;\nmod beta;\n' > crates/rd-api/tests/two/main.rs
echo "#[test] fn a() {}" > crates/rd-api/tests/one/alpha.rs
echo "#[test] fn b() {}" > crates/rd-api/tests/two/beta.rs
mkdir -p crates/rd-api/tests/common
echo "pub fn setup() {}" > crates/rd-api/tests/common/mod.rs
mkdir -p web/src/locales/en web/src/locales/de
echo '{"a": "A"}' > web/src/locales/en/ui.json
echo '{"a": "A"}' > web/src/locales/de/ui.json
echo '{"log": "L"}' > web/src/locales/en/logs.json
echo '{"log": "L"}' > web/src/locales/en/settings.json
echo "import settings from '@/locales/en/settings.json'" > web/src/settings.test.ts
echo "<template />" > web/src/App.vue
git add -A
git commit -qm "the green state"
green="$(git rev-parse 'HEAD^{tree}')"

# shellcheck source=../lib/verified.sh
source "$repo/scripts/lib/verified.sh"
# shellcheck source=../lib/scope.sh
source "$repo/scripts/lib/scope.sh"
# shellcheck source=../lib/check-reuse.sh
source "$repo/scripts/lib/check-reuse.sh"
# shellcheck source=../lib/check-scope.sh
source "$repo/scripts/lib/check-scope.sh"
# shellcheck source=../lib/crate-graph.sh
source "$repo/scripts/lib/crate-graph.sh"
# shellcheck source=../lib/check-reuse-crates.sh
source "$repo/scripts/lib/check-reuse-crates.sh"

skipped=()
skip() { skipped+=("$1 — $2"); }
touches() { [[ -n "$changed" ]] && grep -qE "$1" <<< "$changed"; }
rd_record_full "$SCRATCH/integration" rust "$green"
rd_record_full "$SCRATCH/integration" web "$green"
rd_record_full "$SCRATCH/integration" preflight "$green"
# A green git cannot compare any more: passed over.
rd_record_full "$SCRATCH/gone" rust 0123456789abcdef0123456789abcdef01234567

# check.sh --full $@ over the working state, up to the tests: the reuse plan, the scope, the per
# crate step. Its plan lands in $plan, the reuse plan's status (0: covered, it ends) in $plan_status.
ROOT="$repo"
full=1
plan_run() {
    plan_status=0
    skipped=()
    unset web_locales_typecheck
    rd_check_reuse_plan "$repo" --full "$@" > /dev/null || plan_status=$?
    run_rust=1 run_web=1
    for argument in "$@"; do
        case "$argument" in --rust) run_web=0 ;; --web) run_rust=0 ;; esac
    done
    rd_check_reuse_apply
    changed="" boundary="$(git rev-parse HEAD)" full_tree="$(rd_worktree_tree "$repo")"
    rd_check_scope > /dev/null
    rd_check_reuse_crates > "$SCRATCH/plan"
    plan="$(cat "$SCRATCH/plan")"
}
reset() { git checkout -q -- . && git clean -qfd; }
change() { local path; for path in "$@"; do mkdir -p "$(dirname "$path")"; echo "// changed" >> "$path"; done; }
expect_line() { if grep -qF -- "$2" <<< "$3"; then expect "$1" x x; else expect "$1" "$2" "$3"; fi; }
rust_summary() { echo "${wide_reason:-} | ${test_packages[*]} | lib ${rd_api_lib} | ${rd_api_binaries[*]} | crash ${failpoints} ${crash_packages[*]} | sqlx ${sqlx}"; }

# --- the graph -----------------------------------------------------------------------------------
rd_crate_load_graph
expect "the members, globs expanded" "base guest leaf mid rd-api rd-core top" \
    "$(printf '%s\n' "${RD_CRATE_NAME[@]}" | LC_ALL=C sort | tr '\n' ' ' | sed 's/ $//')"
expect "the reverse hull follows every edge, dev-dependencies and plugins too" "base guest leaf mid rd-api top" \
    "$(rd_crate_reverse_hull base | tr '\n' ' ' | sed 's/ $//')"
expect "a leaf's hull is itself" "leaf" "$(rd_crate_reverse_hull leaf)"

# --- Rust per crate --------------------------------------------------------------------------------
change crates/leaf/tests/it.rs
plan_run
expect "a test change in a leaf crate: only that crate" " | leaf | lib 0 |  | crash 0  | sqlx 0" "$(rust_summary)"
expect_line "the plan names the green it builds on" "builds on the --full green of tree ${green:0:12}" "$plan"
expect_line "and what each path demands" "crates/leaf/tests/it.rs -> the tests of leaf" "$plan"
expect_line "and how many members it checks" "checks 1 of 7 members: leaf" "$plan"
expect_line "the rest is listed under skipped, and why" "tests of 6 of 7 workspace members — the --full green of tree ${green:0:12} covers them" "${skipped[*]}"
expect_line "rd-api's library is left to it too" "the --full green of tree ${green:0:12} covers them" "$rd_api_lib_skip_reason"
reset

change crates/leaf/src/parse_tests.rs
plan_run
expect "a test module behind #[cfg(test)]: only its crate" " | leaf | lib 0 |  | crash 0  | sqlx 0" "$(rust_summary)"
reset

change crates/base/src/helpers_tests.rs
plan_run
expect "a *_tests.rs that is not behind #[cfg(test)]: the hull" " | base guest leaf mid top | lib 1 | one two | crash 1 mid rd-api | sqlx 1" "$(rust_summary)"
reset

change crates/base/src/lib.rs
plan_run
expect "a crate with dependants: the whole reverse hull, rd-api, its crash runs and sqlx" \
    " | base guest leaf mid top | lib 1 | one two | crash 1 mid rd-api | sqlx 1" "$(rust_summary)"
expect "every rd-api suite" "alpha beta" "${rd_api_selected[*]}"
reset

sed -i 's/assert_eq!(1, 1);/assert_eq!(2, 2);/' crates/base/src/lib.rs
plan_run
expect "an edit inside a #[cfg(test)] mod block: only its crate" " | base | lib 0 |  | crash 0  | sqlx 0" "$(rust_summary)"
expect_line "named as test code" "crates/base/src/lib.rs -> the tests of base" "$plan"
sed -i 's/^mod tests {$/mod tests {\n    use super::*;/' crates/base/src/lib.rs
plan_run
expect "and a line added to its top" " | base | lib 0 |  | crash 0  | sqlx 0" "$(rust_summary)"
reset

sed -i 's/^#\[cfg(test)\]$/#[cfg(any(test, feature = "x"))]/' crates/base/src/lib.rs
plan_run
expect "an edit of the block's gate: the hull" " | base guest leaf mid top | lib 1 | one two | crash 1 mid rd-api | sqlx 1" "$(rust_summary)"
reset

sed -i 's/^pub fn f() {}$/pub fn f() { let _ = 1; }/' crates/base/src/lib.rs
sed -i 's/assert_eq!(1, 1);/assert_eq!(2, 2);/' crates/base/src/lib.rs
plan_run
expect "an edit inside the block and one outside: the hull" " | base guest leaf mid top | lib 1 | one two | crash 1 mid rd-api | sqlx 1" "$(rust_summary)"
reset

change crates/mid/src/lib.rs
plan_run
expect "a crate in the middle: it and what builds on it, not what it builds on" " | mid top | lib 1 | one two | crash 1 mid rd-api | sqlx 1" "$(rust_summary)"
reset

change crates/leaf/tests/fixtures/page.html
plan_run
expect "a fixture another crate's library reads: that crate's hull too" " | leaf mid top | lib 1 | one two | crash 1 mid rd-api | sqlx 1" "$(rust_summary)"
expect_line "named in the plan" "crates/mid/src/lib.rs -> mid and what builds on it" "$plan"
reset

change crates/leaf/tests/data/only.html
plan_run
expect "a fixture only a comment elsewhere names: its crate's tests" " | leaf | lib 0 |  | crash 0  | sqlx 0" "$(rust_summary)"
reset

change crates/rd-api/tests/two/beta.rs
plan_run
expect "an rd-api suite file: its binary alone, no library" " |  | lib 0 | two | crash 0  | sqlx 0" "$(rust_summary)"
expect "the binary runs whole, unfiltered" "beta |" "${rd_api_selected[*]} |${rd_api_filter[*]+ ${rd_api_filter[*]}}"
reset

change crates/rd-api/tests/one/alpha.rs
plan_run
expect "a suite the crash list runs: that run too" " |  | lib 0 | one | crash 1 rd-api | sqlx 0" "$(rust_summary)"
reset

change crates/rd-api/tests/common/mod.rs
plan_run
expect "rd-api's shared test code: every binary" " |  | lib 1 | one two | crash 1 rd-api | sqlx 0" "$(rust_summary)"
reset

change web/src/locales/en/logs.json
plan_run
expect "a path the Rust test inputs map names: those crates' hull" " | leaf | lib 0 |  | crash 0  | sqlx 0" "$(rust_summary)"
reset

# --- the whole half -------------------------------------------------------------------------------
for path in Cargo.lock crates/mid/Cargo.toml crates/rd-core/src/lib.rs crates/leaf/build.rs rust-toolchain.toml; do
    change "$path"
    plan_run
    expect "$path: the whole Rust half" "--full" "$wide_reason"
    expect_line "saying why" "the whole half — since the green of tree ${green:0:12}, $path" "$plan"
    reset
done
mkdir -p crates/stray
echo "fn main() {}" > crates/stray/main.rs
plan_run
expect "a Rust input no member holds and no crate is known to read: the whole half" "--full" "$wide_reason"
reset

change crates/leaf/src/lib.rs
plan_run --again
expect "--again: everything" "--full" "$wide_reason"
expect "and says nothing about a green" "" "$plan"
reset

# The closest green wins: a second checkout's green of a later state.
git checkout -q -b later
change crates/mid/src/lib.rs
git commit -qam "mid moved on"
rd_record_full "$SCRATCH/later" rust "$(git rev-parse 'HEAD^{tree}')"
change crates/leaf/tests/it.rs
plan_run
expect "the green that differs least is the one built on" " | leaf | lib 0 |  | crash 0  | sqlx 0" "$(rust_summary)"
reset
git checkout -q development
rm -f "$(rd_full_marker "$SCRATCH/later")"

# --- web per kind -----------------------------------------------------------------------------------
change web/src/locales/de/ui.json
plan_run
expect "translation catalogues only: Vitest alone" "1 0" "$web_locales_only ${web_locales_typecheck:-0}"
expect_line "the plan says so" "builds on the web green of tree ${green:0:12}" "$plan"
expect "the Rust half stays covered" "0" "$run_rust"
reset

change web/src/locales/en/settings.json
plan_run
expect "a catalogue a source imports by name: the typecheck too" "1 1" "$web_locales_only ${web_locales_typecheck:-0}"
reset

change web/src/locales/de/ui.json web/src/App.vue
plan_run
expect "a catalogue and a component: the whole web half" "0" "$web_locales_only"
reset

change web/src/locales/en/logs.json
plan_run
expect "a catalogue a crate compiles in: Vitest for the web half, the crate for the Rust half" "1 leaf" \
    "$web_locales_only ${test_packages[*]}"
reset

# --- nothing changed --------------------------------------------------------------------------------
plan_run
expect "nothing changed since the green: the run ends before the lock, as before" "0" "$plan_status"

finish_tests check-reuse-crates
