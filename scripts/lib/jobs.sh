# shellcheck shell=bash
#
# The parallelism of every script in scripts/, in one place (RD-130-17).
#
# Usage, after ROOT is known:
#
#     source "$ROOT/scripts/lib/jobs.sh"
#
# Environment (a value that is already set wins, so `JOBS=2 scripts/check.sh` keeps working):
#   JOBS          cargo's build jobs (`-j`, CARGO_BUILD_JOBS). This bounds memory: the peak is
#                 in rustc and the link, one of each per job. Default 4.
#   TEST_THREADS  nextest's `--test-threads` when running tests. Default: twice JOBS, never more
#                 than the machine has cores (RD-140-06). Building is over by the time a test
#                 runs, and the memory peak this caps JOBS for is rustc and the link, not a test
#                 process; what a test thread costs is a core. Twice the build width keeps the
#                 cores busy while tests wait on sockets, SQLite and timers, and the core count is
#                 the ceiling past which more threads only queue.
#   RD_VITEST_WORKERS  Vitest's `--maxWorkers` while check.sh runs the web half beside the Rust
#                 half (RD-1270-23); alone (`--web`, a skipped Rust half) and in CI Vitest keeps
#                 its own default, the cores less one. Default: the cores TEST_THREADS leaves,
#                 at most TEST_THREADS, at least 2 — on this machine 8 of 32. Beside nextest,
#                 Vitest's 31 vmThreads workers with jsdom ran seven tests past their 20 s limit
#                 (integration/1.27-w1, 2026-10-12); alone, 8 workers took as long as 31 (19 s).
#
# Deliberately capped and not derived from nproc. This machine reports 32 cores, and letting
# cargo use them exhausted memory and took WSL down more than once; the release profile uses
# thin LTO with codegen-units=1, so the peak is in the link step regardless of core count. The
# default moves only after a measurement (RD-130-17, "Stabilität vor Tempo").
#
# Not exported: a script that starts another passes JOBS on explicitly, as it always has, and a
# value the caller exported reaches every child anyway.

: "${JOBS:=4}"
rd_jobs_cores="$(nproc 2> /dev/null || getconf _NPROCESSORS_ONLN 2> /dev/null || true)"
[[ "$rd_jobs_cores" =~ ^[1-9][0-9]*$ ]] || rd_jobs_cores=""
# Derived only from a JOBS that is a number; anything else is left to the check below to report.
if [[ -z "${TEST_THREADS:-}" && ! "$JOBS" =~ ^[1-9][0-9]*$ ]]; then
    TEST_THREADS="$JOBS"
elif [[ -z "${TEST_THREADS:-}" ]]; then
    TEST_THREADS=$((JOBS * 2))
    if [[ -n "$rd_jobs_cores" && "$rd_jobs_cores" -lt "$TEST_THREADS" ]]; then
        TEST_THREADS="$rd_jobs_cores"
    fi
fi
# Likewise from a TEST_THREADS that is a number.
if [[ -z "${RD_VITEST_WORKERS:-}" && ! "$TEST_THREADS" =~ ^[1-9][0-9]*$ ]]; then
    RD_VITEST_WORKERS="$TEST_THREADS"
elif [[ -z "${RD_VITEST_WORKERS:-}" ]]; then
    RD_VITEST_WORKERS="$TEST_THREADS"
    if [[ -n "$rd_jobs_cores" && $((rd_jobs_cores - TEST_THREADS)) -lt "$RD_VITEST_WORKERS" ]]; then
        RD_VITEST_WORKERS=$((rd_jobs_cores - TEST_THREADS))
    fi
    if [[ "$RD_VITEST_WORKERS" -lt 2 ]]; then RD_VITEST_WORKERS=2; fi
fi
unset rd_jobs_cores

# A typo here would reach cargo as `-j` and fail late, or as 0 and mean something else.
for rd_jobs_name in JOBS TEST_THREADS RD_VITEST_WORKERS; do
    if [[ ! "${!rd_jobs_name}" =~ ^[1-9][0-9]*$ ]]; then
        echo "$rd_jobs_name must be a positive whole number, not '${!rd_jobs_name}'" >&2
        exit 2
    fi
done
unset rd_jobs_name
