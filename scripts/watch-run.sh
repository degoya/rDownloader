#!/usr/bin/env bash
#
# Follows a long run's log without reading all of it (RD-1100-13): prints each stage start
# (`==> …`), each failure — a `!! …` line, a failing test (nextest's FAIL, TIMEOUT and signal
# lines), a compiler error — and the end, and returns once process <pid> has ended. A full run
# writes thousands of lines; following them with `tail -F` wakes a watcher on every one of them,
# and the few that matter drown.
#
#   scripts/watch-run.sh <pid> <log>...
#   scripts/watch-run.sh "$(cat /tmp/claude-1000/integration-1.10-w1/check.pid)" \
#       /tmp/claude-1000/integration-1.10-w1/windows.log /tmp/claude-1000/integration-1.10-w1/check.log
#
# Several logs are followed together, for a run that writes one after the other (integrate.sh's
# Windows lint, then --full); a log that does not exist yet is waited for. The verdict at the end
# reads each log by its kind (RD-1300-03), as AGENTS.md says to judge a run: check.sh's — any log
# with a `==> ` line — by its closing line `==> all requested checks passed`; public-ci.sh's, told
# by its opening line, by its own green after its last opening (integrate.sh adds a second run),
# both lines from scripts/lib/public-ci.sh; any other log by its last `REAL EXIT`, saying so. Each
# verdict names the `REAL EXIT` the run wrote, and a non-zero one is never green. Exit 0 when every
# log ends green, 1 otherwise, 2 on a usage error.
#
# Nothing here judges by the filtered lines: they are progress. The verdict is the closing line.
set -euo pipefail
# shellcheck source=lib/public-ci.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib/public-ci.sh"

usage() { echo "usage: scripts/watch-run.sh <pid> <log>..." >&2; exit 2; }
[[ $# -ge 2 && "$1" =~ ^[1-9][0-9]*$ ]] || usage
pid="$1"
shift

# The stage starts, the failures, the closing lines. `==> ` is check.sh's stage line and every
# summary line; `!! ` every failure check.sh and the scripts it calls report.
pattern='^==> |^!! |^ *(FAIL|TIMEOUT|SIG[A-Z]+|LEAK-FAIL) \[|^error(\[[A-Za-z0-9]+\])?: |^REAL EXIT: |^test .* \.\.\. FAILED$'

# -F follows a log through its creation; --pid ends tail once the run has ended (it reads what is
# left first); -q leaves out the per-file headers, which would read as stage lines.
tail -q -n +1 -F --pid="$pid" "$@" 2> /dev/null | grep --line-buffered -E "$pattern" || true

# The last `REAL EXIT` of log $1, empty when the run wrote none.
real_exit() { { grep '^REAL EXIT: ' "$1" || true; } | tail -n 1 | sed 's/^REAL EXIT: //'; }

# Whether public-ci log $1 ends green: its own green line after its last opening line, and no
# `REAL EXIT` but 0.
public_ci_green() {
    OPENING="$RD_PUBLIC_CI_LOG_OPENING" GREEN="$RD_PUBLIC_CI_LOG_GREEN" awk '
        $0 ~ ENVIRON["OPENING"] { green = 0 } $0 ~ ENVIRON["GREEN"] { green = 1 } END { exit !green }' "$1" \
        && [[ "$(real_exit "$1")" =~ ^0?$ ]]
}

verdict=0
for log in "$@"; do
    if [[ ! -f "$log" ]]; then
        echo "-- $log: never written"
        verdict=1
    elif grep -qE "$RD_PUBLIC_CI_LOG_OPENING" "$log"; then
        code="$(real_exit "$log")"
        if public_ci_green "$log"; then
            echo "-- $log: green on GitHub${code:+ (exit $code)}"
        else
            echo "-- $log: NOT green on GitHub${code:+ (exit $code)}"
            verdict=1
        fi
    elif ! grep -q '^==> ' "$log"; then
        code="$(real_exit "$log")"
        if [[ "$code" == 0 ]]; then
            echo "-- $log: green by its REAL EXIT alone, no closing line of a known kind (exit 0)"
        else
            echo "-- $log: NOT green, no closing line of a known kind (exit ${code:-none written})"
            verdict=1
        fi
    elif grep -qx '==> all requested checks passed' "$log"; then
        echo "-- $log: green$(grep -m1 '^REAL EXIT: ' "$log" | sed 's/^REAL EXIT: / (exit /; s/$/)/')"
    else
        echo "-- $log: NOT green$(grep '^REAL EXIT: ' "$log" | tail -n 1 | sed 's/^REAL EXIT: / (exit /; s/$/)/')"
        verdict=1
    fi
done
exit "$verdict"
