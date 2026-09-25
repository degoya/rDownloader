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
#   TEST_THREADS  nextest's `--test-threads` when running tests. This bounds CPU only — building
#                 is over by then — so it may be set higher than JOBS. Default: JOBS.
#
# Deliberately capped and not derived from nproc. This machine reports 32 cores, and letting
# cargo use them exhausted memory and took WSL down more than once; the release profile uses
# thin LTO with codegen-units=1, so the peak is in the link step regardless of core count. The
# default moves only after a measurement (RD-130-17, "Stabilität vor Tempo").
#
# Not exported: a script that starts another passes JOBS on explicitly, as it always has, and a
# value the caller exported reaches every child anyway.

: "${JOBS:=4}"
: "${TEST_THREADS:=$JOBS}"

# A typo here would reach cargo as `-j` and fail late, or as 0 and mean something else.
for rd_jobs_name in JOBS TEST_THREADS; do
    if [[ ! "${!rd_jobs_name}" =~ ^[1-9][0-9]*$ ]]; then
        echo "$rd_jobs_name must be a positive whole number, not '${!rd_jobs_name}'" >&2
        exit 2
    fi
done
unset rd_jobs_name
