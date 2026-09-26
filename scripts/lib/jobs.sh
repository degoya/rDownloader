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
#
# Deliberately capped and not derived from nproc. This machine reports 32 cores, and letting
# cargo use them exhausted memory and took WSL down more than once; the release profile uses
# thin LTO with codegen-units=1, so the peak is in the link step regardless of core count. The
# default moves only after a measurement (RD-130-17, "Stabilität vor Tempo").
#
# Not exported: a script that starts another passes JOBS on explicitly, as it always has, and a
# value the caller exported reaches every child anyway.

: "${JOBS:=4}"
# Derived only from a JOBS that is a number; anything else is left to the check below to report.
if [[ -z "${TEST_THREADS:-}" && ! "$JOBS" =~ ^[1-9][0-9]*$ ]]; then
    TEST_THREADS="$JOBS"
elif [[ -z "${TEST_THREADS:-}" ]]; then
    TEST_THREADS=$((JOBS * 2))
    rd_jobs_cores="$(nproc 2> /dev/null || getconf _NPROCESSORS_ONLN 2> /dev/null || echo "$TEST_THREADS")"
    if [[ "$rd_jobs_cores" =~ ^[1-9][0-9]*$ && "$rd_jobs_cores" -lt "$TEST_THREADS" ]]; then
        TEST_THREADS="$rd_jobs_cores"
    fi
    unset rd_jobs_cores
fi

# A typo here would reach cargo as `-j` and fail late, or as 0 and mean something else.
for rd_jobs_name in JOBS TEST_THREADS; do
    if [[ ! "${!rd_jobs_name}" =~ ^[1-9][0-9]*$ ]]; then
        echo "$rd_jobs_name must be a positive whole number, not '${!rd_jobs_name}'" >&2
        exit 2
    fi
done
unset rd_jobs_name
