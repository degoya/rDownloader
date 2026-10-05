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
# reads each log's closing line, as AGENTS.md says to judge a run — `==> all requested checks
# passed` — and its `REAL EXIT` line when the run wrote one. Exit 0 when every log ends green,
# 1 otherwise, 2 on a usage error.
#
# Nothing here judges by the filtered lines: they are progress. The verdict is the closing line.
set -euo pipefail

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

verdict=0
for log in "$@"; do
    if [[ ! -f "$log" ]]; then
        echo "-- $log: never written"
        verdict=1
    elif grep -qx '==> all requested checks passed' "$log"; then
        echo "-- $log: green$(grep -m1 '^REAL EXIT: ' "$log" | sed 's/^REAL EXIT: / (exit /; s/$/)/')"
    else
        echo "-- $log: NOT green$(grep '^REAL EXIT: ' "$log" | tail -n 1 | sed 's/^REAL EXIT: / (exit /; s/$/)/')"
        verdict=1
    fi
done
exit "$verdict"
