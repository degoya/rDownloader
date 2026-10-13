#!/usr/bin/env bash
#
# What a check.sh --full shares with the preflight — the file checks, cargo fmt, both test maps,
# gitleaks and the script checks — is not run again over a tree a recorded preflight green covers
# (RD-1280-13), in a scratch checkout with every check a stand-in that logs its call: with the
# green each is one skip line and only the job layout and the version copies run, which read more
# than the tree; without it all of them run; after a source change all of them run again; a change
# to a generated file alone, which the generators write after integrate.sh's preflight, stays
# covered; and a red Rust test after the covered shared stages is the Rust half's, which is then not
# recorded (the 1.28 integration recorded it green as the preflight's failure).
#
#   scripts/tests/full-preflight-covered.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
export CARGO_TARGET_DIR="$SCRATCH/target"
export FAKE="$SCRATCH/fake"
unset GITLEAKS RD_SKIP_JOB_LAYOUT RD_RELEASE_VERSION
mkdir -p "$FAKE" "$SCRATCH/bin"

# A stand-in that logs `<name> <args>`, on PATH or in the checkout.
stand_in() {
    printf '#!/usr/bin/env bash\necho "%s $*" >> "$FAKE/calls"\n%s\n' "$2" "${3:-}" > "$1"
    chmod +x "$1"
}
stand_in "$SCRATCH/bin/cargo" cargo
stand_in "$SCRATCH/bin/gitleaks" gitleaks
stand_in "$SCRATCH/bin/shellcheck" shellcheck '[[ "$1" != --version ]] || { echo "version: 0.0.0"; exit 0; }'
stand_in "$SCRATCH/bin/actionlint" actionlint '[[ "$1" != --version ]] || { echo "0.0.0"; exit 0; }'
export PATH="$SCRATCH/bin:$PATH"

TREE="$SCRATCH/checkout"
git init -q -b development "$TREE"
mkdir -p "$TREE/scripts/lib" "$TREE/scripts/tests" "$TREE/web" "$TREE/crates/rd-x/src"
cp "$ROOT/scripts/public-exclude.txt" "$TREE/scripts/"
cp "$ROOT"/scripts/lib/*.sh "$TREE/scripts/lib/"
: > "$TREE/scripts/lib/rd-api-tests.map"
: > "$TREE/scripts/lib/rust-test-inputs.map"
printf 'import sys\nsys.exit(0)\n' > "$TREE/scripts/lib/rust-test-inputs.py"
for name in archive-jobs set-version tool-versions check-actions-pinned plugin-release-notes release-notes \
    release-sections docker-tools; do
    stand_in "$TREE/scripts/$name.sh" "$name.sh"
done
stand_in "$TREE/scripts/tests/a.sh" a.sh
echo "fn x() {}" > "$TREE/crates/rd-x/src/lib.rs"
echo '{"info": {"version": "1.0.0"}}' > "$TREE/web/openapi.json"
printf '[workspace.package]\nversion = "1.0.0"\n' > "$TREE/Cargo.toml"
git -C "$TREE" add -A
git -C "$TREE" commit -qm base

# The shared stages of a --full as check.sh runs them, after its reuse plan (lib/check-reuse.sh).
full_stages() {
    rm -f "$FAKE/calls"
    run_status bash -c '
        set -euo pipefail
        cd "$1"
        CHECK_LOGS="$2/logs" RUN_KIND=full full=1 changed="" ROOT="$1"
        source scripts/lib/stages.sh
        source scripts/lib/scope.sh
        source scripts/lib/verified.sh
        source scripts/lib/check-reuse.sh
        source scripts/lib/script-checks.sh
        source scripts/lib/public.sh
        source scripts/lib/preflight.sh
        source scripts/lib/check-record.sh
        rd_stages_init
        rd_check_reuse_plan "$1" --full > /dev/null || true
        preflight_cover="$(rd_preflight_cover_reason)"
        rd_file_checks "$(git rev-parse HEAD)" "$preflight_cover"
        rd_script_checks
        rd_secret_scan "$preflight_cover"
        rd_rust_tree_checks "$preflight_cover"
        # A Rust test stage after them, red while $FAKE/rust-tests-red exists, and the halves
        # recorded as check.sh records them.
        step "tests"
        attempt bash -c "echo rust-tests >> \"\$FAKE/calls\"; [[ ! -f \"\$FAKE/rust-tests-red\" ]]"
        full_tree="$(rd_worktree_tree "$1")" run_rust=1 covered_rust=0 run_web=0 covered_web=0
        rd_stages_report
        rd_check_record_halves
    ' run "$TREE" "$SCRATCH"
}
ran() { grep -q "^$1" "$FAKE/calls" 2> /dev/null; }
shared="cargo|gitleaks|shellcheck|actionlint|a\.sh|check-actions-pinned|plugin-release-notes|release-notes|release-sections|docker-tools"
reason="a recorded preflight green covers this tree up to generated files and documentation"

# --- no preflight green: everything runs ----------------------------------------------------------
full_stages
expect_status "without a preflight green" 0
expect_true "cargo fmt, gitleaks, the script tests and every file check run" \
    'ran "cargo fmt --all --check" && ran gitleaks && ran a.sh && ran check-actions-pinned.sh && ran docker-tools.sh && ran release-sections.sh'
expect_true "and the test maps" 'grep -q "^==> the rd-api test map" <<< "$output" && grep -q "^==> the Rust test inputs map" <<< "$output"'
expect_true "and no skip names the preflight" '! grep -q "$reason" <<< "$output"'

# --- a preflight green of this tree: one skip line each ---------------------------------------------
(cd "$TREE" && source scripts/lib/verified.sh && rd_record_full "$TREE" preflight "$(rd_worktree_tree "$TREE")")
full_stages
expect_status "with a preflight green of this tree" 0
expect_true "nothing the green covers runs again" "! grep -qE '^($shared)' \"\$FAKE/calls\""
expect_true "the job layout and the version copies still run, they read more than the tree" \
    'ran "archive-jobs.sh --check" && ran "set-version.sh --check"'
expect_output "the file checks: one skip line" \
    "- the file checks that read the tree alone (6: git diff --check, the action pins, both release-note checks, the released sections, the container's Python tools) — $reason"
expect_output "cargo fmt and the maps: one" "- cargo fmt and both test maps — $reason"
expect_output "gitleaks: one" "- gitleaks — $reason"
expect_output "the script checks: one" "- actionlint, bash -n, shellcheck and the script tests — $reason"
expect_true "and no test map ran" '! grep -q "^==> the rd-api test map" <<< "$output"'

# --- covered, and a Rust test red: the Rust half's failure, not recorded --------------------------
touch "$FAKE/rust-tests-red"
full_stages
expect_true "the shared stages still skipped" "! grep -qE '^($shared)' \"\$FAKE/calls\""
expect_true "the Rust test ran" 'ran rust-tests'
expect_output "the rust half is named as failed, only the preflight half recorded" \
    "==> recorded the preflight half for tree $(cd "$TREE" && source scripts/lib/verified.sh && rd_worktree_tree "$TREE" | cut -c1-12) in"
expect_true "the rust half is not recorded" '! grep -q "recorded the rust" <<< "$output" && grep -q "although the rust half failed" <<< "$output"'
rm "$FAKE/rust-tests-red"

# --- a generated file alone changed since: still covered ------------------------------------------
echo '{"info": {"version": "1.0.0"}, "paths": {}}' > "$TREE/web/openapi.json"
full_stages
expect_true "a generated file is not what the preflight reads: still skipped" "! grep -qE '^($shared)' \"\$FAKE/calls\""
git -C "$TREE" checkout -q -- web/openapi.json

# --- a source changed since: everything runs again -------------------------------------------------
echo "fn y() {}" >> "$TREE/crates/rd-x/src/lib.rs"
full_stages
expect_status "after a source change" 0
expect_true "cargo fmt, gitleaks, the script tests and the file checks run again" \
    'ran "cargo fmt --all --check" && ran gitleaks && ran a.sh && ran check-actions-pinned.sh'
expect_true "with no skip naming the preflight" '! grep -q "$reason" <<< "$output"'

finish_tests full-preflight-covered
