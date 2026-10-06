#!/usr/bin/env bash
#
# scripts/ci-rust-tests.sh, the test step of ci.yml's `rust` job, against a stub `cargo`: on Linux
# five groups, the rd-api integration binaries named from crates/rd-api/tests/*/main.rs, each
# group's executables deleted once it ran and libraries kept, every group run to its end and the
# script failing at the end; on Windows one `--workspace` group (RD-1120-07); `--no-run` without
# `--no-fail-fast`, which nextest refuses beside it.
#
# Pure bash. check.sh runs it when scripts/ change, and under --full.
#
#   scripts/tests/ci-rust-tests.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

# cargo: logs its arguments one call per line, links one test executable and one library per
# call, writes a JUnit report, and fails for the selection named in $FAKE/fail.
export FAKE="$SCRATCH/fake"
mkdir -p "$FAKE" "$SCRATCH/bin"
cat > "$SCRATCH/bin/cargo" <<'EOF'
#!/usr/bin/env bash
echo "$*" >> "$FAKE/cargo.calls"
calls="$(wc -l < "$FAKE/cargo.calls")"
mkdir -p target/debug/deps target/nextest/ci
sleep 0.01
touch "target/debug/deps/test_$calls-0123" "target/debug/deps/test_$calls-0123.pdb" \
    "target/debug/deps/libdep_$calls.rlib"
echo "<testsuites/>" > target/nextest/ci/junit.xml
[[ -f "$FAKE/fail" && "$*" == *"$(cat "$FAKE/fail")"* ]] && exit 101
exit 0
EOF
chmod +x "$SCRATCH/bin/cargo"
export PATH="$SCRATCH/bin:$PATH"

WORK="$SCRATCH/work"
mkdir -p "$WORK/crates/rd-api/tests/"{access,queue,common}
touch "$WORK/crates/rd-api/tests/access/main.rs" "$WORK/crates/rd-api/tests/queue/main.rs"
export RUNNER_TEMP="$SCRATCH/temp"
# Runs the script on runner OS $1 with the rest as its arguments, from a clean target/.
tests_on() {
    local os="$1"
    shift
    rm -rf "$WORK/target" "$RUNNER_TEMP" "$FAKE/cargo.calls"
    mkdir -p "$RUNNER_TEMP"
    run_status bash -c "cd '$WORK' && RUNNER_OS='$os' '$ROOT/scripts/ci-rust-tests.sh' $*"
}

# --- Linux: the groups ---------------------------------------------------------------------------
tests_on Linux
expect_status "Linux, all green" 0
expect "five groups, the rd-api suites by their directories" \
    "nextest run -P ci --no-fail-fast --workspace --exclude rd-api --exclude rd-plugin-ext --exclude rdownloader|nextest run -P ci --no-fail-fast -p rd-plugin-ext|nextest run -P ci --no-fail-fast -p rdownloader|nextest run -P ci --no-fail-fast -p rd-api --lib|nextest run -P ci --no-fail-fast -p rd-api --test access --test queue" \
    "$(paste -sd'|' "$FAKE/cargo.calls")"
expect "every group's executables and what is named after them deleted" "" \
    "$(cd "$WORK/target/debug/deps" && ls test_* 2> /dev/null || true)"
expect "the libraries kept" "5" "$(find "$WORK/target/debug/deps" -name 'libdep_*' | wc -l)"
expect "a JUnit report per group" "5" "$(find "$RUNNER_TEMP/junit" -name 'group-*.xml' | wc -l)"

echo "-p rdownloader" > "$FAKE/fail"
tests_on Linux
expect_status "a failed group fails the script" 1
expect "and every group after it still runs" "5" "$(wc -l < "$FAKE/cargo.calls")"
rm -f "$FAKE/fail"

tests_on Linux --no-run
expect_status "--no-run" 0
expect_true "builds without --no-fail-fast" \
    '! grep -q -- --no-fail-fast "$FAKE/cargo.calls" && [[ $(grep -c -- "-P ci --no-run" "$FAKE/cargo.calls") -eq 5 ]]'

# --- Windows: one group (RD-1120-07) -------------------------------------------------------------
tests_on Windows
expect_status "Windows, green" 0
expect "one --workspace group" "nextest run -P ci --no-fail-fast --workspace" "$(cat "$FAKE/cargo.calls")"
echo "--workspace" > "$FAKE/fail"
tests_on Windows
expect_status "Windows, red: fails" 1
rm -f "$FAKE/fail"
tests_on Windows --no-run
expect "Windows --no-run builds the same one group" "nextest run -P ci --no-run --workspace" "$(cat "$FAKE/cargo.calls")"

tests_on Linux --bogus
expect_status "an unknown argument is refused" 2

finish_tests "ci-rust-tests"
