#!/usr/bin/env bash
#
# The lanes of scripts/lib/lock.sh and the target derivation of scripts/lib/lanes.sh (RD-140-06),
# against a stand-in job script and a scratch repository.
#
# The job script sources the real lock library and does nothing heavy: it writes `start`/`end`
# lines to an event log and, depending on its mode, waits for a partner, holds its lane, exits
# with a given code, leaves a background process behind or calls itself nested. The cases pin
# what the lanes promise: two target directories build at once while a lane is free, one target
# directory never does, RD_LANES=1 is the old single lock, the script's exit code comes through
# untouched (196 included, the code a busy lane uses), giving up exits 199 without running, a
# child process never inherits a lock, and a fresh target is not stamped.
#
# Pure bash, git and flock, no cargo: it runs in about twenty seconds. check.sh runs it when
# scripts/lib/ or scripts/tests/ change, and under --full.
#
#   scripts/tests/lock-lanes.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

command -v flock > /dev/null || { echo "lock-lanes: flock is not installed"; exit 1; }

failures=0
passed=0
ok() { echo "ok   $1"; passed=$((passed + 1)); }
fail() { echo "FAIL $1"; shift; printf '     %s\n' "$@"; failures=$((failures + 1)); }

# --- the stand-in job ------------------------------------------------------------------------

JOB_ROOT="$SCRATCH/job"
mkdir -p "$JOB_ROOT/scripts" "$JOB_ROOT/crates/demo/src"
cat > "$JOB_ROOT/scripts/job.sh" <<'JOB'
#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=/dev/null
source "$RD_TEST_LIB/lock.sh"
rd_take_lock "$@"
name="$1" mode="$2" arg="${3:-}"
echo "start $name" >> "$RD_TEST_DIR/events"
case "$mode" in
    meet)
        : > "$RD_TEST_DIR/$name.in"
        for _ in $(seq 30); do [[ -e "$RD_TEST_DIR/$arg.in" ]] && break; sleep 0.1; done
        sleep 0.3 ;;
    hold) sleep "$arg" ;;
    exit) echo run >> "$RD_TEST_DIR/$name.runs"; exit "$arg" ;;
    daemon) sleep "$arg" > /dev/null 2>&1 & ;;
    nested) "$0" "$name-inner" quick ;;
    quick) ;;
esac
echo "end $name" >> "$RD_TEST_DIR/events"
JOB
chmod +x "$JOB_ROOT/scripts/job.sh"

export RD_TEST_LIB="$ROOT/scripts/lib" RD_TEST_DIR="$SCRATCH/run"
export RD_LOCK_FILE="$SCRATCH/lock" RD_MIN_FREE_MB=0 RD_NO_SCCACHE=1 RD_LANE_POLL=1
unset RD_LOCK_HELD RD_LOCK_STAGE RD_LOCK_STARTED RD_LOCK_LANE RD_LOCK_NO_SLOT RD_NO_LOCK \
    RD_LANE_TARGET_DIR CARGO_TARGET_DIR RUSTC_WRAPPER

fresh() { rm -rf "$RD_TEST_DIR"; mkdir -p "$RD_TEST_DIR"; : > "$RD_TEST_DIR/events"; }
# job <target> <args…>: one run of the stand-in, building in target directory <target>.
job() { local target="$1"; shift; CARGO_TARGET_DIR="$SCRATCH/$target" "$JOB_ROOT/scripts/job.sh" "$@"; }
events() { tr '\n' ' ' < "$RD_TEST_DIR/events" | sed 's/ $//'; }
# Whether the event log shows two runs that never overlapped.
serial() {
    local e; e="$(events)"
    [[ "$e" == "start $1 end $1 start $2 end $2" || "$e" == "start $2 end $2 start $1 end $1" ]]
}

# --- lanes -----------------------------------------------------------------------------------

fresh
RD_LANES=2 job t-a a meet b > "$RD_TEST_DIR/a.out" 2>&1 & first=$!
RD_LANES=2 job t-b b meet a > "$RD_TEST_DIR/b.out" 2>&1 & second=$!
wait "$first"; wait "$second"
if [[ "$(events)" =~ ^start\ .\ start ]]; then
    ok "two targets, two lanes: both build at once"
else
    fail "two targets, two lanes: both build at once" "events: $(events)"
fi

fresh
RD_LANES=1 job t-a a meet b > /dev/null 2>&1 & first=$!
sleep 0.2
RD_LANES=1 job t-b b meet a > /dev/null 2>&1 & second=$!
wait "$first"; wait "$second"
if serial a b; then ok "RD_LANES=1: two targets take turns"; else fail "RD_LANES=1: two targets take turns" "events: $(events)"; fi

fresh
RD_LANES=2 job t-a a meet b > /dev/null 2>&1 & first=$!
sleep 0.2
RD_LANES=2 job t-a b meet a > /dev/null 2>&1 & second=$!
wait "$first"; wait "$second"
if serial a b; then ok "one target never builds twice at once"; else fail "one target never builds twice at once" "events: $(events)"; fi

# --- exit codes and giving up ----------------------------------------------------------------

fresh
status=0; RD_LANES=2 job t-a a exit 3 > /dev/null 2>&1 || status=$?
[[ "$status" -eq 3 ]] && ok "the script's exit code comes through" || fail "the script's exit code comes through" "got $status"

fresh
status=0; RD_LANES=2 job t-a a exit 196 > /dev/null 2>&1 || status=$?
runs="$(wc -l < "$RD_TEST_DIR/a.runs")"
if [[ "$status" -eq 196 && "$runs" -eq 1 ]]; then
    ok "a script exiting 196 is not taken for a busy lane"
else
    fail "a script exiting 196 is not taken for a busy lane" "exit $status, ran $runs time(s)"
fi

fresh
RD_LANES=1 job t-a a hold 4 > /dev/null 2>&1 & first=$!
sleep 0.5
status=0; RD_LANES=1 RD_LOCK_WAIT=1 job t-b b quick > "$RD_TEST_DIR/b.out" 2>&1 || status=$?
wait "$first"
if [[ "$status" -eq 199 ]] && ! grep -q 'start b' "$RD_TEST_DIR/events"; then
    ok "no free lane within RD_LOCK_WAIT: exit 199, nothing run"
else
    fail "no free lane within RD_LOCK_WAIT: exit 199, nothing run" "exit $status, events: $(events)"
fi

fresh
RD_LANES=2 job t-a a hold 4 > /dev/null 2>&1 & first=$!
sleep 0.5
status=0; RD_LANES=2 RD_LOCK_WAIT=1 job t-a b quick > /dev/null 2>&1 || status=$?
wait "$first"
if [[ "$status" -eq 199 ]] && ! grep -q 'start b' "$RD_TEST_DIR/events"; then
    ok "target busy within RD_LOCK_WAIT: exit 199, nothing run"
else
    fail "target busy within RD_LOCK_WAIT: exit 199, nothing run" "exit $status, events: $(events)"
fi

# --- inheritance and nesting -----------------------------------------------------------------

# Without `flock -o` the background process keeps the lock's descriptor and with it the lock —
# what the sccache server would do to every run after the first.
fresh
RD_LANES=1 job t-a a daemon 6 > /dev/null 2>&1
status=0; RD_LANES=1 RD_LOCK_WAIT=2 job t-a b quick > /dev/null 2>&1 || status=$?
[[ "$status" -eq 0 ]] && ok "a process the run leaves behind holds no lock" \
    || fail "a process the run leaves behind holds no lock" "the next run exited $status"

fresh
status=0; RD_LANES=1 RD_LOCK_WAIT=2 job t-a a nested > /dev/null 2>&1 || status=$?
if [[ "$status" -eq 0 && "$(events)" == "start a start a-inner end a-inner end a" ]]; then
    ok "a nested call passes through the held lock"
else
    fail "a nested call passes through the held lock" "exit $status, events: $(events)"
fi

# --- stamping --------------------------------------------------------------------------------

source_file="$JOB_ROOT/crates/demo/src/lib.rs"
: > "$source_file"
touch -d '2001-01-01' "$source_file"
fresh
RD_LANES=1 job t-fresh a quick > /dev/null 2>&1
if [[ "$(stat -c %Y "$source_file")" -lt 1000000000 && "$(cat "$SCRATCH/t-fresh/.rd-checkout")" == "$JOB_ROOT" ]]; then
    ok "a fresh target is claimed without stamping"
else
    fail "a fresh target is claimed without stamping" "mtime $(stat -c %Y "$source_file"), marker $(cat "$SCRATCH/t-fresh/.rd-checkout" 2>&1)"
fi

mkdir -p "$SCRATCH/t-used/debug/.fingerprint"
echo /somewhere/else > "$SCRATCH/t-used/.rd-checkout"
fresh
RD_LANES=1 job t-used a quick > /dev/null 2>&1
if [[ "$(stat -c %Y "$source_file")" -gt 1000000000 && "$(cat "$SCRATCH/t-used/.rd-checkout")" == "$JOB_ROOT" ]]; then
    ok "a target another checkout built in is stamped"
else
    fail "a target another checkout built in is stamped" "mtime $(stat -c %Y "$source_file")"
fi

# --- the target derivation and worktree.sh new --own-target ----------------------------------

REPO="$SCRATCH/repo"
mkdir -p "$REPO/scripts/lib" "$REPO/web"
cp "$ROOT/scripts/worktree.sh" "$REPO/scripts/"
cp "$ROOT"/scripts/lib/*.sh "$REPO/scripts/lib/"
: > "$REPO/web/.keep"
git -C "$REPO" init -q -b main
git -C "$REPO" -c user.name=t -c user.email=t@t add -A
git -C "$REPO" -c user.name=t -c user.email=t@t commit -qm base
# worktree.sh installs web/node_modules with pnpm (RD-150-14); a stand-in, nothing to install.
mkdir -p "$SCRATCH/bin"
printf '#!/usr/bin/env bash\nexit 0\n' > "$SCRATCH/bin/pnpm"
chmod +x "$SCRATCH/bin/pnpm"
for branch in feat/shared "--own-target feat/own"; do
    # shellcheck disable=SC2086 # the flag and the branch are two words on purpose
    PATH="$SCRATCH/bin:$PATH" BASE=main "$REPO/scripts/worktree.sh" new $branch > "$SCRATCH/worktree.out" 2>&1 \
        || { echo "FAIL worktree.sh new $branch"; sed 's/^/     /' "$SCRATCH/worktree.out"; exit 1; }
done
# Beside the repository, so inside the scratch directory the trap removes.
shared="$REPO-feat-shared" own="$REPO-feat-own"

# shellcheck source=../lib/lanes.sh
source "$ROOT/scripts/lib/lanes.sh"
check_dir() {
    local label="$1" expected="$2" got="$3"
    [[ "$got" == "$expected" ]] && ok "$label" || fail "$label" "expected $expected" "got      $got"
}
check_dir "the main checkout builds in its target/" "$REPO/target" "$(rd_target_dir "$REPO")"
check_dir "a worktree shares the main target/" "$REPO/target" "$(rd_target_dir "$shared")"
check_dir "a worktree with --own-target builds in its own" "$own/target" "$(rd_target_dir "$own")"
check_dir "CARGO_TARGET_DIR wins" "/elsewhere" "$(CARGO_TARGET_DIR=/elsewhere rd_target_dir "$own")"
check_dir "the own target links the shared components" "$REPO/target/wasm32-unknown-unknown" \
    "$(readlink "$own/target/wasm32-unknown-unknown")"
check_dir "the shared target keeps the lock file everyone knows" "$RD_LOCK_FILE" \
    "$(rd_target_lock_file "$REPO/target" "$shared")"
own_lock="$(rd_target_lock_file "$own/target" "$own")"
[[ "$own_lock" == "$RD_LOCK_FILE.target-"* ]] && ok "an own target has a lock of its own" \
    || fail "an own target has a lock of its own" "got $own_lock"
if rd_target_is_fresh "$own/target"; then
    ok "linked components do not make a target used"
else
    fail "linked components do not make a target used" "rd_target_is_fresh said no for $own/target"
fi

mkdir -p "$REPO/target/lanes/windows"
expected="$(printf '%s\n' "$REPO/target" "$REPO/target/lanes/windows" "$own/target")"
check_dir "every target directory, lanes and own targets included" "$expected" "$(rd_all_target_dirs "$shared")"

cp "$ROOT/scripts/prune-target.sh" "$REPO/scripts/"
pruned="$("$REPO/scripts/prune-target.sh" --dry-run 2>&1 | grep -c '^==> would free' || true)"
pruned_all="$("$REPO/scripts/prune-target.sh" --dry-run --all 2>&1 | grep -c '^==> would free' || true)"
if [[ "$pruned" -eq 2 && "$pruned_all" -eq 3 ]]; then
    ok "prune-target.sh: the target and its lanes, --all adds the own targets"
else
    fail "prune-target.sh: the target and its lanes, --all adds the own targets" \
        "pruned $pruned target(s), with --all $pruned_all; expected 2 and 3"
fi

# --- the release chain's two packages --------------------------------------------------------

PIPE="$SCRATCH/pipe"
mkdir -p "$PIPE/scripts/lib"
cp "$ROOT/scripts/release-pipeline.sh" "$PIPE/scripts/"
cp "$ROOT"/scripts/lib/*.sh "$PIPE/scripts/lib/"
git -C "$PIPE" init -q -b development
git -C "$PIPE" -c user.name=t -c user.email=t@t add -A
git -C "$PIPE" -c user.name=t -c user.email=t@t commit -qm base
PIPE_LOG="$PIPE/artifacts/release-evidence-9.9.9.log"

# pipeline <windows exit>: sources the chain as a library, replaces the two package steps with
# stand-ins that wait for each other, and runs them the way the chain does.
pipeline() {
    (
        export RELEASE_PIPELINE_LIB=1 RD_LANES=2 WINDOWS_EXIT="$1"
        set -- 9.9.9
        # shellcheck source=../release-pipeline.sh
        source "$PIPE/scripts/release-pipeline.sh"
        step_build_linux() {
            : > "$RD_TEST_DIR/linux.in"
            for _ in $(seq 30); do [[ -e "$RD_TEST_DIR/windows.in" ]] && break; sleep 0.1; done
            [[ -e "$RD_TEST_DIR/windows.in" ]] && echo "linux built beside windows"
        }
        step_build_windows() {
            : > "$RD_TEST_DIR/windows.in"
            for _ in $(seq 30); do [[ -e "$RD_TEST_DIR/linux.in" ]] && break; sleep 0.1; done
            [[ -e "$RD_TEST_DIR/linux.in" ]] && echo "windows built beside linux"
            return "$WINDOWS_EXIT"
        }
        run_steps_parallel build-linux build-windows
    )
}
marker() { grep "^##RD-STEP id=$1 " "$PIPE_LOG" | sed -n "s/.* $2=\([^ ]*\).*/\1/p"; }

fresh
status=0; pipeline 0 > "$RD_TEST_DIR/pipe.out" 2>&1 || status=$?
if [[ "$status" -eq 0 && "$(marker build-linux exit)" == 0 && "$(marker build-windows exit)" == 0 \
    && "$(marker build-linux bytes)" -gt 0 && "$(marker build-windows bytes)" -gt 0 ]] \
    && grep -q 'linux built beside windows' "$PIPE_LOG" && grep -q 'windows built beside linux' "$PIPE_LOG"; then
    ok "release chain: both packages at once, each with its own evidence"
else
    fail "release chain: both packages at once, each with its own evidence" "exit $status" \
        "$(grep '^##RD-STEP' "$PIPE_LOG" 2>&1)"
fi

fresh
status=0; pipeline 5 > "$RD_TEST_DIR/pipe.out" 2>&1 || status=$?
if [[ "$status" -eq 5 && "$(marker build-linux exit)" == 0 && "$(marker build-windows exit)" == 5 ]]; then
    ok "release chain: a failed package stops the chain with its own exit"
else
    fail "release chain: a failed package stops the chain with its own exit" "exit $status" \
        "$(grep '^##RD-STEP' "$PIPE_LOG" 2>&1)"
fi

echo
if [[ "$failures" -gt 0 ]]; then
    echo "lock-lanes: $failures failed, $passed passed"
    exit 1
fi
echo "lock-lanes: $passed passed"
