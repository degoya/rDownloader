#!/usr/bin/env bash
#
# scripts/check.sh --preflight (RD-1110-15) in a scratch checkout, every stage a stand-in that
# logs its call: a red run lists every red stage at once — git diff --check, the job layout, cargo
# fmt, both test maps, gitleaks, shellcheck, a script test — after every stage ran, and records no
# green; cargo is asked to format and nothing else; gitleaks sees the new files of the working tree
# and .gitleaks.toml, not what scripts/public-exclude.txt leaves out; RD_SKIP_JOB_LAYOUT=1 skips the
# job layout with its reason; --preflight runs alone.
#
#   scripts/tests/check-preflight.sh
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
unset GITLEAKS RD_SKIP_JOB_LAYOUT
mkdir -p "$FAKE" "$SCRATCH/bin"

# A stand-in on PATH or in the checkout: logs `<name> <args>` and fails while $FAKE/<name>-red
# exists, after the lines of $3 (shell code).
stand_in() {
    printf '#!/usr/bin/env bash\necho "%s $*" >> "$FAKE/calls"\n%s\nif [[ -f "$FAKE/%s-red" ]]; then exit 3; fi\n' \
        "$2" "${3:-}" "$2" > "$1"
    chmod +x "$1"
}
stand_in "$SCRATCH/bin/cargo" cargo
stand_in "$SCRATCH/bin/shellcheck" shellcheck '[[ "$1" != --version ]] || { echo "version: 0.0.0"; exit 0; }'
stand_in "$SCRATCH/bin/actionlint" actionlint '[[ "$1" != --version ]] || { echo "0.0.0"; exit 0; }'
stand_in "$SCRATCH/bin/gitleaks" gitleaks '
[[ ! -e docs ]] || echo "gitleaks saw docs/" >> "$FAKE/calls"
[[ ! -f .gitleaks.toml ]] || echo "gitleaks read .gitleaks.toml" >> "$FAKE/calls"
[[ ! -f leak.txt ]] || { echo "leaks found: 1"; exit 1; }'
export PATH="$SCRATCH/bin:$PATH"

TREE="$SCRATCH/checkout"
git init -q -b development "$TREE"
mkdir -p "$TREE/scripts/lib" "$TREE/scripts/tests" "$TREE/docs"
cp "$ROOT/scripts/check.sh" "$ROOT/scripts/public-exclude.txt" "$TREE/scripts/"
cp "$ROOT"/scripts/lib/*.sh "$TREE/scripts/lib/"
: > "$TREE/scripts/lib/rd-api-tests.map"
printf 'import os, sys\nsys.exit(1 if os.path.exists(os.environ["FAKE"] + "/inputs-red") else 0)\n' \
    > "$TREE/scripts/lib/rust-test-inputs.py"
stand_in "$TREE/scripts/archive-jobs.sh" archive-jobs.sh
stand_in "$TREE/scripts/set-version.sh" set-version.sh
stand_in "$TREE/scripts/check-actions-pinned.sh" check-actions-pinned.sh
stand_in "$TREE/scripts/tests/a.sh" a.sh
stand_in "$TREE/scripts/tests/b.sh" b.sh
echo "internal" > "$TREE/docs/notes.md"
echo 'title = "scratch"' > "$TREE/.gitleaks.toml"
echo "text" > "$TREE/text.txt"
git -C "$TREE" add -A
git -C "$TREE" commit -qm base

# Never the real check: should --preflight ever fall through, a lock of its own that is never
# waited for keeps it away from the real one, and cargo is the stand-in.
preflight() {
    run_status env -u RD_LOCK_HELD RD_LOCK_FILE="$SCRATCH/lock" RD_LOCK_WAIT=0 \
        RD_CHECK_LOGS="$SCRATCH/logs" "$@" "$TREE/scripts/check.sh" --preflight
}

# --- every stage red at once ------------------------------------------------------------------
for name in archive-jobs.sh cargo shellcheck a.sh; do touch "$FAKE/$name-red"; done
touch "$FAKE/inputs-red"
mkdir -p "$TREE/crates/rd-api/tests"
echo "fn main() {}" > "$TREE/crates/rd-api/tests/stray.rs"
echo "trailing   " >> "$TREE/text.txt"
echo "a new file" > "$TREE/leak.txt"
preflight
FAILURES="$SCRATCH/logs/failures"
expect_status "a red preflight ends non-zero" 1
expect_output "after every stage ran, the last script test too" "==> script test: scripts/tests/b.sh"
expect_true "without the closing line" '! grep -q "all requested checks passed" <<< "$output"'
expect_output "and records no green" "No green was recorded."
expect "every red stage in the failure list, in one run" \
    "git diff --check|the job layout: finished jobs archived, open ones not|cargo fmt --all --check|the rd-api test map against the test suites|the Rust test inputs map against the sources|gitleaks over the tree the public export would publish|shellcheck 0.0.0 over N shell scripts (severity warning)|script test: scripts/tests/a.sh" \
    "$(sed -n '/ — exit /{s/ — exit .*//; s/ over [0-9]* shell/ over N shell/; p}' "$FAILURES" | paste -sd'|' -)"
expect "the green ones are not in it" "0" "$(grep -cE '^(the version|the workflows|bash -n|actionlint|script test: scripts/tests/b)' "$FAILURES" || true)"
expect "cargo formats and builds nothing" "cargo fmt --all --check" "$(grep '^cargo' "$FAKE/calls")"
expect_true "gitleaks scans the new file and reads .gitleaks.toml" \
    'grep -qx "gitleaks dir . --no-banner --redact --exit-code 1" "$FAKE/calls" && grep -qx "gitleaks read .gitleaks.toml" "$FAKE/calls"'
expect_true "but not what the export leaves out" '! grep -q "gitleaks saw docs/" "$FAKE/calls"'

# --- green, and the job layout left to integrate.sh's generator --------------------------------
rm -f "$FAKE"/*-red "$FAKE/calls" "$TREE/leak.txt" "$TREE/crates/rd-api/tests/stray.rs"
git -C "$TREE" checkout -q -- text.txt
preflight
expect_status "a green preflight" 0
expect_output "to its closing line" "==> all requested checks passed"
expect_output "saying it records no green" "no green is recorded"
expect_true "the job layout was checked" 'grep -qx "archive-jobs.sh --check" "$FAKE/calls"'

rm -f "$FAKE/calls"
preflight RD_SKIP_JOB_LAYOUT=1
expect_status "with RD_SKIP_JOB_LAYOUT=1" 0
expect_true "the job layout is not checked" '! grep -q "^archive-jobs.sh" "$FAKE/calls"'
expect_output "and listed as skipped, with the reason" "- the job layout — RD_SKIP_JOB_LAYOUT=1"

run_status env -u RD_LOCK_HELD RD_LOCK_FILE="$SCRATCH/lock" RD_LOCK_WAIT=0 \
    "$TREE/scripts/check.sh" --preflight --full
expect_status "--preflight with another flag is a usage error" 2
expect_output "saying it runs alone" "--preflight runs alone"

finish_tests check-preflight
