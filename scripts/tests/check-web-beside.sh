#!/usr/bin/env bash
#
# The web half of scripts/check.sh beside the Rust half (RD-1270-13, scripts/lib/check-web.sh),
# with pnpm and the extension build as stand-ins: when it builds no web/dist it starts before the
# Rust half and runs beside it, its output printed whole after it, its stages, skips and failures
# joined to the run's under logs of their own, Vitest bounded to RD_VITEST_WORKERS (RD-1270-23); a
# run that would build web/dist, or RD_WEB_SERIAL=1, keeps the order — the web half after the Rust
# half, Vitest at its own default.
#
#   scripts/tests/check-web-beside.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

export FAKE="$SCRATCH/fake"
mkdir -p "$FAKE" "$SCRATCH/bin" "$SCRATCH/checkout/scripts" "$SCRATCH/checkout/web"
# pnpm: logs its call, marks that the web half started, red for typecheck while $FAKE/typecheck-red
# exists.
cat > "$SCRATCH/bin/pnpm" <<'EOF'
#!/usr/bin/env bash
: > "$FAKE/web-started"
echo "pnpm $*" >> "$FAKE/calls"
echo "output of pnpm $*"
if [[ "$*" == *typecheck* && -f "$FAKE/typecheck-red" ]]; then echo "error TS2304: a finding"; exit 3; fi
EOF
printf '#!/usr/bin/env bash\necho "build-extension" >> "$FAKE/calls"\n' > "$SCRATCH/checkout/scripts/build-extension.sh"
# Exit 0: web/dist is newer than its sources; 1 (with $FAKE/stale): it would be built.
printf '#!/usr/bin/env bash\n[[ ! -f "$FAKE/stale" ]]\n' > "$SCRATCH/checkout/scripts/web-dist-stale.sh"
chmod +x "$SCRATCH/bin/pnpm" "$SCRATCH/checkout/scripts/"*.sh
export PATH="$SCRATCH/bin:$PATH"
# What scripts/lib/jobs.sh derives in check.sh.
export RD_VITEST_WORKERS=3

# One run: the halves as check.sh orders them, the Rust half a stage that waits up to two seconds
# for the web half to start (met/alone). Prints the run's output; leaves the stage lists in
# $SCRATCH/state.
run() {
    rm -rf "$SCRATCH/logs" "$FAKE/calls" "$FAKE/web-started"
    (
        cd "$SCRATCH/checkout"
        CHECK_LOGS="$SCRATCH/logs" RUN_KIND=full
        # shellcheck source=../lib/stages.sh
        source "$ROOT/scripts/lib/stages.sh"
        # shellcheck source=../lib/check-web.sh
        source "$ROOT/scripts/lib/check-web.sh"
        rd_stages_init
        run_rust=1 run_web=1 web_changed=1 extension_changed=1
        rd_check_web_start
        step "the Rust half"
        attempt bash -c 'for _ in $(seq 20); do [[ -e "$FAKE/web-started" ]] && { echo met; exit 0; }; sleep 0.1; done; echo alone'
        rd_check_web_join
        close_stage
        {
            printf 'stage %s\n' "${stage_names[@]}"
            printf 'skipped %s\n' "${skipped[@]}"
            printf 'failed %s\n' "${failed_stages[@]+"${failed_stages[@]}"}"
        } > "$SCRATCH/state"
    ) 2>&1
}

# --- beside: web/dist is the main checkout's link, so nothing builds it --------------------------
ln -s /nonexistent "$SCRATCH/checkout/web/dist"
touch "$FAKE/typecheck-red"
output="$(run)"
expect_true "the web half started before the Rust half ended" 'grep -qx met "$SCRATCH/logs/full-01.log"'
expect_output "it says so" "==> the web half runs beside the Rust half"
expect_true "its output follows the Rust half's, whole" \
    '[[ "$(grep -n "the Rust half$" <<< "$output" | head -n 1 | cut -d: -f1)" -lt "$(grep -n "output of pnpm --dir web run typecheck:full" <<< "$output" | cut -d: -f1)" ]]'
expect "its stages join the run's, in order" \
    "stage the Rust half|stage pnpm run typecheck:full|stage pnpm run test --maxWorkers=3|stage browser extension" \
    "$(grep '^stage ' "$SCRATCH/state" | paste -sd'|' -)"
expect_true "its skips too" 'grep -q "^skipped pnpm run build — a feature worktree" "$SCRATCH/state"'
expect "its failure is the run's" "failed pnpm run typecheck:full" "$(grep '^failed ' "$SCRATCH/state")"
expect_true "in the failure list, with its own stage log" \
    'grep -q "^pnpm run typecheck:full — exit 3, log $SCRATCH/logs/full-web-01.log" "$SCRATCH/logs/failures" && grep -q "output of pnpm --dir web run typecheck:full" "$SCRATCH/logs/full-web-01.log"'
expect_true "the Rust half's log holds none of it" '! grep -q pnpm "$SCRATCH/logs/full-01.log"'
expect_true "Vitest gets RD_VITEST_WORKERS workers" 'grep -qx "pnpm --dir web run test --maxWorkers=3" "$FAKE/calls"'
expect_output "and the run names them" "Vitest with 3 workers (RD_VITEST_WORKERS)"

# --- after: RD_WEB_SERIAL=1 -----------------------------------------------------------------------
rm -f "$FAKE/typecheck-red"
output="$(RD_WEB_SERIAL=1 run)"
expect_true "RD_WEB_SERIAL=1: the web half after the Rust half" 'grep -qx alone "$SCRATCH/logs/full-01.log"'
expect "with the usual stage logs" "pnpm --dir web run typecheck:full" "$(grep -o 'pnpm --dir web run typecheck:full' "$SCRATCH/logs/full-02.log" | head -n 1)"
expect "and nothing failed" "failed " "$(grep '^failed' "$SCRATCH/state")"
expect_true "Vitest at its own default" 'grep -qx "pnpm --dir web run test" "$FAKE/calls"'
expect_true "under the plain stage name" 'grep -qx "stage pnpm run test" "$SCRATCH/state"'

# --- after: a run that builds web/dist, which rust-embed compiles in ------------------------------
rm "$SCRATCH/checkout/web/dist"
mkdir "$SCRATCH/checkout/web/dist"
touch "$FAKE/stale"
output="$(run)"
expect_true "a web/dist to build: the web half after the Rust half" 'grep -qx alone "$SCRATCH/logs/full-01.log"'
expect_true "and the build ran" 'grep -qx "pnpm --dir web run build" "$FAKE/calls"'
expect_true "Vitest at its own default there too" 'grep -qx "pnpm --dir web run test" "$FAKE/calls"'

rm -f "$FAKE/stale"
output="$(run)"
expect_true "a web/dist newer than its sources: beside again" 'grep -qx met "$SCRATCH/logs/full-01.log"'

# --- a run that ends before the join takes the web half along -----------------------------------
rm -f "$FAKE/calls"
printf '#!/usr/bin/env bash\necho "build-extension" >> "$FAKE/calls"\necho $$ > "$FAKE/extension.pid"\nexec sleep 30\n' \
    > "$SCRATCH/checkout/scripts/build-extension.sh"
output="$(
    exec 2>&1
    cd "$SCRATCH/checkout"
    CHECK_LOGS="$SCRATCH/logs" RUN_KIND=full
    # shellcheck source=../lib/stages.sh
    source "$ROOT/scripts/lib/stages.sh"
    # shellcheck source=../lib/check-web.sh
    source "$ROOT/scripts/lib/check-web.sh"
    rd_stages_init
    run_rust=1 run_web=1 web_changed=0 extension_changed=1
    rd_check_web_start
    for _ in $(seq 50); do [[ -s "$FAKE/extension.pid" ]] && break; sleep 0.1; done
    exit 1
)" || true
sleep 0.3
expect_output "an early end says it stopped the web half" "!! the web half was stopped"
expect_true "and nothing of it runs on" '! kill -0 "$(cat "$FAKE/extension.pid")" 2> /dev/null'

finish_tests check-web-beside
