#!/usr/bin/env bash
#
# The parallelism defaults of scripts/lib/jobs.sh, on a stand-in `nproc`: TEST_THREADS twice JOBS
# capped at the cores (RD-140-06), and Vitest's workers beside the Rust half (RD-1270-23) — the
# cores TEST_THREADS leaves, at most TEST_THREADS, at least 2; a value already set wins, and a value
# that is no positive whole number stops the script.
#
#   scripts/tests/jobs.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
# shellcheck source=lib/expect.sh
source "$ROOT/scripts/tests/lib/expect.sh"

# nproc and getconf answer $FAKE_CORES, or fail when it is empty.
mkdir -p "$SCRATCH/bin"
printf '#!/usr/bin/env bash\n[[ -n "${FAKE_CORES:-}" ]] && echo "$FAKE_CORES"\n' > "$SCRATCH/bin/nproc"
cp "$SCRATCH/bin/nproc" "$SCRATCH/bin/getconf"
chmod +x "$SCRATCH/bin/nproc" "$SCRATCH/bin/getconf"

# Sources jobs.sh with the cores $1 and the assignments after it; prints JOBS TEST_THREADS
# RD_VITEST_WORKERS.
derive() {
    local cores="$1"
    shift
    env -u JOBS -u TEST_THREADS -u RD_VITEST_WORKERS PATH="$SCRATCH/bin:$PATH" FAKE_CORES="$cores" "$@" \
        bash -c 'set -eu; source "$1"; echo "$JOBS $TEST_THREADS $RD_VITEST_WORKERS"' _ "$ROOT/scripts/lib/jobs.sh"
}

expect "32 cores: 4 jobs, 8 test threads, 8 Vitest workers" "4 8 8" "$(derive 32)"
expect "12 cores: Vitest takes the 4 the tests leave" "4 8 4" "$(derive 12)"
expect "8 cores: the tests take them all, Vitest still 2" "4 8 2" "$(derive 8)"
expect "4 cores: the tests capped at them, Vitest 2" "4 4 2" "$(derive 4)"
expect "64 cores and JOBS=8: Vitest as wide as the tests" "8 16 16" "$(derive 64 JOBS=8)"
expect "TEST_THREADS=20 on 32 cores: Vitest the 12 left" "4 20 12" "$(derive 32 TEST_THREADS=20)"
expect "no core count: Vitest as wide as the tests" "4 8 8" "$(derive '')"
expect "RD_VITEST_WORKERS set wins" "4 8 5" "$(derive 32 RD_VITEST_WORKERS=5)"

run_status derive 32 RD_VITEST_WORKERS=0
expect_status "RD_VITEST_WORKERS=0 stops the script" 2
expect_output "and says why" "RD_VITEST_WORKERS must be a positive whole number, not '0'"
run_status derive 32 TEST_THREADS=x
expect_status "a TEST_THREADS that is no number stops it" 2
expect_output "named as itself, not as the Vitest workers derived from it" "TEST_THREADS must be a positive whole number, not 'x'"

finish_tests jobs
