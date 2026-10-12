#!/usr/bin/env bash
#
# scripts/check.sh --gate (RD-1100-13, RD-1270-13) in a scratch checkout with cargo a stand-in:
# the Linux and the Windows lint run side by side, the Windows one in a target directory of its
# own (cargo's build-directory lock would serialise the two in one), each with its own stage log;
# a red run lists both findings at once and records no green, a green one records both halves;
# RD_GATE_SERIAL=1 runs them one after the other. A lock of its own keeps it from the real one.
#
#   scripts/tests/check-gate.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

command -v flock > /dev/null || { echo "check-gate: flock is not installed; skipped"; exit 0; }

export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
export CARGO_TARGET_DIR="$SCRATCH/target" FAKE="$SCRATCH/fake"
export RD_LOCK_FILE="$SCRATCH/lock" RD_LOCK_WAIT=5 RD_MIN_FREE_MB=0 RD_NO_SCCACHE=1 \
    RD_CHECK_LOGS="$SCRATCH/logs"
unset RD_LOCK_HELD RD_LOCK_STAGE RD_LOCK_STARTED RD_LOCK_LANE RD_LOCK_NO_SLOT RD_NO_LOCK \
    RD_LANE_TARGET_DIR RUSTC_WRAPPER RD_GATE_SERIAL
mkdir -p "$FAKE" "$SCRATCH/bin"

# cargo: `clippy` is the Linux lint, `xwin clippy` the Windows one. Each logs its target
# directory, waits up to 1.2 seconds for the other to start (met/alone), and fails with a
# compiler-style finding while $FAKE/<linux|windows>-red exists.
cat > "$SCRATCH/bin/cargo" <<'EOF'
#!/usr/bin/env bash
[[ "$1 ${2:-}" != "xwin --version" ]] || { echo "cargo-xwin 0.0.0"; exit 0; }
kind=linux other=windows
[[ "$1" != xwin ]] || { kind=windows other=linux; }
echo "$kind target $CARGO_TARGET_DIR" >> "$FAKE/calls"
: > "$FAKE/$kind.in"
met=alone
for _ in $(seq 12); do
    [[ -e "$FAKE/$other.in" ]] && { met=met; sleep 0.3; break; }
    sleep 0.1
done
rm -f "$FAKE/$kind.in"
echo "$kind $met" >> "$FAKE/calls"
echo "    Checking rd-core ($kind)"
if [[ -f "$FAKE/$kind-red" ]]; then
    echo "error: a $kind finding"
    echo "  --> crates/rd-core/src/$kind.rs:1:1"
    exit 101
fi
EOF
chmod +x "$SCRATCH/bin/cargo"
export PATH="$SCRATCH/bin:$PATH"

TREE="$SCRATCH/checkout"
git init -q -b development "$TREE"
mkdir -p "$TREE/scripts/lib"
cp "$ROOT/scripts/check.sh" "$TREE/scripts/"
cp "$ROOT"/scripts/lib/*.sh "$TREE/scripts/lib/"
git -C "$TREE" add -A
git -C "$TREE" commit -qm base

gate() { run_status env "$@" "$TREE/scripts/check.sh" --gate; }
calls() { cat "$FAKE/calls" 2> /dev/null || true; }

# --- both red, side by side --------------------------------------------------------------------
touch "$FAKE/linux-red" "$FAKE/windows-red"
gate
expect_status "a red gate ends non-zero" 1
expect_true "the two lints ran side by side" 'grep -qx "linux met" "$FAKE/calls" && grep -qx "windows met" "$FAKE/calls"'
expect_true "the Linux lint in the checkout's target" 'grep -qx "linux target $CARGO_TARGET_DIR" "$FAKE/calls"'
expect_true "the Windows lint in a directory of its own" \
    'grep -qx "windows target $CARGO_TARGET_DIR/lanes/windows-lint" "$FAKE/calls"'
expect_true "each with its own stage log" \
    'grep -q "(linux)" "$RD_CHECK_LOGS/gate-01.log" && grep -q "(windows)" "$RD_CHECK_LOGS/gate-02.log" && ! grep -q "(windows)" "$RD_CHECK_LOGS/gate-01.log"'
expect_true "both findings in the one failure list" \
    'grep -q "^the Linux lint — exit 101" "$RD_CHECK_LOGS/failures" && grep -q "^the Windows lint — exit 101" "$RD_CHECK_LOGS/failures" && grep -q "error: a windows finding" "$RD_CHECK_LOGS/failures"'
expect_output "both outputs in the run's own output, one after the other" "==> the Windows lint: exit 101"
expect_true "and no green recorded" '! grep -qs "^clippy \|^windows " "$CARGO_TARGET_DIR/.rd-verified-full/"*'

# --- one red: the other half's green is kept ----------------------------------------------------
rm -f "$FAKE/calls" "$FAKE/windows-red"
gate
expect_status "the Linux lint red" 1
expect "only the Windows half recorded" "windows" "$(cut -d' ' -f1 "$CARGO_TARGET_DIR/.rd-verified-full/"* | paste -sd' ' -)"

# --- RD_GATE_SERIAL=1: one after the other ------------------------------------------------------
rm -f "$FAKE/calls" "$FAKE/linux-red"
gate RD_GATE_SERIAL=1
expect_status "RD_GATE_SERIAL=1, green" 0
expect "only the half no green covers runs" "linux alone" "$(grep -E '^(linux|windows) (met|alone)$' "$FAKE/calls" | paste -sd'|' -)"

rm -f "$FAKE/calls"
run_status "$TREE/scripts/check.sh" --gate --again
expect_status "--again, side by side again" 0
expect_true "both ran, together" 'grep -qx "linux met" "$FAKE/calls" && grep -qx "windows met" "$FAKE/calls"'

rm -f "$FAKE/calls"
run_status env RD_GATE_SERIAL=1 "$TREE/scripts/check.sh" --gate --again
expect_status "RD_GATE_SERIAL=1 with --again" 0
expect "one after the other" "linux alone|windows alone" "$(grep -E '^(linux|windows) (met|alone)$' "$FAKE/calls" | paste -sd'|' -)"
expect_output "as stages of the usual kind" "==> the Windows lint"

rm -f "$FAKE/calls"
gate
expect_status "a tree both greens cover" 0
expect "runs nothing" "" "$(calls)"

# The run that holds the lock asks again (RD-1270-13): a green recorded while it waited ends it.
rm -f "$FAKE/calls"
gate RD_LOCK_HELD=1 RD_LOCK_NO_SLOT=1
expect_status "inside the lock, a tree both greens cover" 0
expect_output "ends it there" "(clippy windows); it is not run again"
expect "and runs nothing" "" "$(calls)"

finish_tests check-gate
