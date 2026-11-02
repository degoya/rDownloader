#!/usr/bin/env bash
#
# The release chain's clippy step after the bump (RD-1410-04), along the path a release takes: the
# integration gate records `clippy` for its tree; the release commits the migration baselines
# (Rust test code of rd-db) and the regenerated licence list on top; check.sh --full on that tree
# lints what changed since the gate's green (rd_lint_full, lib/lint.sh) and records `clippy`; the
# version bump; step_clippy (lib/release-steps-build.sh) then repeats nothing. Until 1.40.1 the
# --full linted nothing, no clippy green ever covered the tree before the bump, and every release
# ran the whole workspace's clippy again. Also what the --full lint leaves alone.
#
# Pure git and bash: cargo is a stand-in that writes down what it was asked.
#
#   scripts/tests/release-clippy-reuse.sh
set -euo pipefail

SCRIPTS_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

# shellcheck source=../lib/verified.sh
source "$SCRIPTS_ROOT/scripts/lib/verified.sh"
# shellcheck source=../lib/scope.sh
source "$SCRIPTS_ROOT/scripts/lib/scope.sh"
# shellcheck source=../lib/crate-graph.sh
source "$SCRIPTS_ROOT/scripts/lib/crate-graph.sh"
# shellcheck source=../lib/lint.sh
source "$SCRIPTS_ROOT/scripts/lib/lint.sh"
# shellcheck source=../lib/release-steps-build.sh
source "$SCRIPTS_ROOT/scripts/lib/release-steps-build.sh"
# shellcheck source=lib/expect.sh
source "$SCRIPTS_ROOT/scripts/tests/lib/expect.sh"

export CARGO_TARGET_DIR="$SCRATCH/target"
export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid

# cargo: each call one line in the log, the exit status FAKE_CARGO_EXIT (default 0).
mkdir -p "$SCRATCH/bin"
CARGO_LOG="$SCRATCH/cargo.log"
cat > "$SCRATCH/bin/cargo" << EOF
#!/usr/bin/env bash
printf '%s\n' "\$*" >> "$CARGO_LOG"
exit "\${FAKE_CARGO_EXIT:-0}"
EOF
chmod +x "$SCRATCH/bin/cargo"
export PATH="$SCRATCH/bin:$PATH"
cargo_calls() { cat "$CARGO_LOG" 2> /dev/null || true; }

# check.sh's stage helpers, as far as rd_lint_full uses them.
failed_stages=()
skipped=()
stage_name=""
step() { stage_name="$*"; echo "==> $*"; }
skip() { skipped+=("$1 — $2"); }
attempt() { "$@" || failed_stages+=("$stage_name"); }

# A workspace in the repository's shape: rd-api builds on rd-files, rd-db on nothing.
repo="$SCRATCH/repo"
git init -q -b development "$repo"
cd "$repo"
ROOT="$repo"
JOBS=2
cat > Cargo.toml << 'EOF'
[workspace]
members = ["crates/*"]

[workspace.package]
version = "1.0.0"
edition = "2024"
EOF
printf '[[package]]\nname = "rd-db"\nversion = "1.0.0"\n' > Cargo.lock
crate() {
    mkdir -p "crates/$1/src"
    printf '[package]\nname = "%s"\nversion.workspace = true\n\n[dependencies]\n%s' "$1" "${2:-}" > "crates/$1/Cargo.toml"
    echo "pub fn f() {}" > "crates/$1/src/lib.rs"
}
crate rd-files
crate rd-db
crate rd-api 'rd-files = { path = "../rd-files" }
'
mkdir -p crates/rd-db/tests/database crates/rd-api/tests/queue crates/rd-api/tests/common crates/rd-api/licenses
echo "mod migration_forward;" > crates/rd-db/tests/database/main.rs
echo 'const BASELINES: &[(&str, i64)] = &[("0.9.0", 1), ("1.0.0", 2)];' > crates/rd-db/tests/database/migration_forward.rs
echo "mod common;" > crates/rd-api/tests/queue/main.rs
echo "pub fn helper() {}" > crates/rd-api/tests/common/mod.rs
echo 'const THIRD_PARTY: &str = include_str!("../licenses/third-party.json");' > crates/rd-api/src/lib.rs
echo '[]' > crates/rd-api/licenses/third-party.json
git add -A
git commit -qm "the integration gate's tree"
gate="$(git rev-parse 'HEAD^{tree}')"
reset() { git checkout -q -- . && git clean -qfd; }
tree_now() { rd_worktree_tree "$repo"; }

# --- the release's path -------------------------------------------------------------------------
# The gate's green, recorded by the integration worktree on the same target.
rd_record_full "$SCRATCH/integration" clippy "$gate"
# What the release commits on top before its --full: the baselines, the licence list.
sed -i 's/("0.9.0", 1), ("1.0.0", 2)/("1.0.0", 2), ("1.0.1", 2)/' crates/rd-db/tests/database/migration_forward.rs
echo '[{"name": "serde"}]' > crates/rd-api/licenses/third-party.json
git commit -qam "test(db): migration baselines at 1.0.1"
before="$(git rev-parse 'HEAD^{tree}')"
expect "no clippy green covers the tree before the bump" "" "$(rd_full_covering "$repo" clippy "$before")"
expect "the --full plans rd-db's targets from the gate's green" "$gate"$'\n'"-p rd-db --all-targets" \
    "$(rd_lint_bridge_plan "$repo" "$before")"

# check.sh --full on that tree.
rm -f "$CARGO_LOG"
run_status rd_lint_full "$before"
expect_status "the --full lint passes" 0
expect "it lints rd-db's targets, not the workspace" \
    "clippy -p rd-db --all-targets --all-features -j 2 -- -D warnings" "$(cargo_calls)"
expect "and records clippy for the tree before the bump" "$before" "$(rd_full_covering "$repo" clippy "$before")"

# The version bump, then the chain's clippy step.
sed -i 's/^version = "1\.0\.0"/version = "1.0.1"/' Cargo.toml Cargo.lock
rm -f "$CARGO_LOG"
run_status step_clippy
expect_status "step_clippy passes" 0
expect_output "step_clippy says the workspace clippy is not repeated" \
    "the workspace clippy is not repeated: a green of half 'clippy' covers the tree before the bump"
expect "step_clippy runs no cargo" "" "$(cargo_calls)"
expect "the green is carried to the bumped tree" "$(tree_now)" "$(rd_full_covering "$repo" clippy "$(tree_now)")"
reset

# A red lint records nothing: the chain's clippy step lints the workspace.
git commit -q --allow-empty -m "another commit" # a fresh HEAD, the same tree
echo '// changed' >> crates/rd-db/tests/database/migration_forward.rs
git commit -qam "test(db): one more line"
red="$(git rev-parse 'HEAD^{tree}')"
rm -f "$CARGO_LOG"
FAKE_CARGO_EXIT=1 rd_lint_full "$red" > /dev/null
expect "a red lint fails its stage" "1" "${#failed_stages[@]}"
failed_stages=()
expect "and records nothing" "" "$(rd_full_covering "$repo" clippy "$red")"
sed -i 's/^version = "1\.0\.0"/version = "1.0.1"/' Cargo.toml Cargo.lock
rm -f "$CARGO_LOG"
run_status step_clippy
expect "then step_clippy lints the workspace" \
    "clippy --workspace --all-targets --all-features -j 2 -- -D warnings" "$(cargo_calls)"
reset

# --- what the --full lint plans -----------------------------------------------------------------
plan_for() { git commit -qam "$1"; rd_lint_bridge_plan "$repo" "$(git rev-parse 'HEAD^{tree}')"; }
git checkout -q --detach "$(git commit-tree "$gate" -m gate)"
echo "// test" >> crates/rd-api/tests/queue/main.rs
expect "an rd-api binary's own file: that binary" "$gate"$'\n'"-p rd-api --test queue" "$(plan_for "queue")"

git checkout -q --detach "$(git commit-tree "$gate" -m gate)"
echo "// shared" >> crates/rd-api/tests/common/mod.rs
expect "rd-api's shared test code: out of reach" "" "$(plan_for "common")"

git checkout -q --detach "$(git commit-tree "$gate" -m gate)"
echo "pub fn g() {}" >> crates/rd-files/src/lib.rs
expect "a library rd-api builds on: out of reach" "" "$(plan_for "rd-files")"

git checkout -q --detach "$(git commit-tree "$gate" -m gate)"
echo "pub fn g() {}" >> crates/rd-db/src/lib.rs
expect "a library nothing builds on: its hull, itself" "$gate"$'\n'"-p rd-db --all-targets" "$(plan_for "rd-db")"

git checkout -q --detach "$(git commit-tree "$gate" -m gate)"
sed -i 's/^edition = "2024"/edition = "2021"/' Cargo.toml
expect "the root manifest: out of reach" "" "$(plan_for "manifest")"

git checkout -q --detach "$(git commit-tree "$gate" -m gate)"
echo '[{"name": "tokio"}]' > crates/rd-api/licenses/third-party.json
expect "the licence list alone: the gate's green, nothing to lint" "$gate" "$(plan_for "licences")"
rm -f "$CARGO_LOG"
rd_lint_full "$(git rev-parse 'HEAD^{tree}')" > /dev/null
expect "  recorded without a cargo call" "" "$(cargo_calls)"

git checkout -q --detach "$(git commit-tree "$gate" -m gate)"
echo "// test" >> crates/rd-db/tests/database/main.rs
git commit -qam "rd-db test"
skipped=()
rm -f "$CARGO_LOG"
RD_FULL_CANDIDATES_MAX=0 rd_lint_full "$(git rev-parse 'HEAD^{tree}')" > /dev/null
expect "no green within reach: a skip, no cargo" "1 " "${#skipped[@]} $(cargo_calls)"

finish_tests "release clippy reuse"
