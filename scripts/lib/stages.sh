#!/usr/bin/env bash
# shellcheck shell=bash
#
# The stages of a scripts/check.sh run: their timing, what was skipped and why, and the failures
# (RD-1100-13), kept here so check.sh stays readable.
#
# A stage that fails does not end the run. Its command runs through `attempt`, which writes the
# output to the terminal and to a log of the stage's own, and on a failure records the stage, its
# exit status, its log and the lines that name what failed — nextest's FAIL lines, a compiler's
# errors with their locations — in $CHECK_LOGS/failures, then lets the run go on. The run ends
# non-zero after its last stage, with the list, and records no green. Until 1.10 the first red
# test ended a --full run, and every further failure cost another round of 10 to 16 minutes.
#
# Not every stage goes through it: the gates that make the rest meaningless (the rd-api test map,
# components that do not build) and the script checks, which take seconds and come first, still
# stop the run at once.
#
# Expects from the caller: CHECK_LOGS (a directory) and RUN_KIND (branch, full, windows, gate),
# set before rd_stages_init.
#
#   step "name"            # closes the previous stage, opens this one
#   skip "what" "why"      # listed at the end under "skipped, and why"
#   attempt cmd args...    # runs cmd in the current stage; a failure is recorded, not fatal
#   rd_stages_report       # the time per stage and the skipped list
#   rd_stages_exit_if_failed

stage_names=()
stage_seconds=()
stage_name=""
stage_started=0
skipped=()
failed_stages=()

# The log directory, emptied of this kind's earlier logs and of an earlier failure list.
rd_stages_init() {
    mkdir -p "$CHECK_LOGS"
    rm -f "$CHECK_LOGS/failures" "$CHECK_LOGS/$RUN_KIND"-*.log
}

close_stage() {
    if [[ -n "$stage_name" ]]; then
        stage_names+=("$stage_name")
        stage_seconds+=($((SECONDS - stage_started)))
    fi
    stage_name=""
}

step() { close_stage; stage_name="$*"; stage_started=$SECONDS; echo; echo "==> $*"; }

skip() { skipped+=("$1 — $2"); }

# The lines of stage log $1 that name what failed, each once, at most 40: nextest's FAIL,
# TIMEOUT and signal lines with the timing and the counter cut off, cargo test's `... FAILED`,
# and a compiler's `error` lines with their `-->` locations.
rd_stage_failure_lines() {
    grep -E '^ *(FAIL|TIMEOUT|SIG[A-Z]+|LEAK-FAIL) \[|^test .* \.\.\. FAILED$|^error(\[[A-Za-z0-9]+\])?: |^ +--> ' "$1" \
        | sed -E 's/^ *(FAIL|TIMEOUT|SIG[A-Z]+|LEAK-FAIL) \[[^]]*\] +(\([^)]*\) +)?/\1 /' \
        | awk '!seen[$0]++' | head -n 40 || true
}

attempt() {
    local log status
    log="$CHECK_LOGS/$RUN_KIND-$(printf '%02d' $(( ${#stage_names[@]} + 1 ))).log"
    set +e
    "$@" 2>&1 | tee -a "$log"
    status="${PIPESTATUS[0]}"
    set -e
    [[ "$status" -ne 0 ]] || return 0
    failed_stages+=("$stage_name")
    {
        echo "$stage_name — exit $status, log $log"
        rd_stage_failure_lines "$log" | sed 's/^/    /'
    } >> "$CHECK_LOGS/failures"
    echo "!! $stage_name failed (exit $status); the run goes on and lists it at the end" >&2
}

rd_stages_report() {
    local index total=0 line
    close_stage
    echo
    echo "==> time per stage"
    for index in "${!stage_names[@]}"; do
        printf '    %5ds  %s\n' "${stage_seconds[$index]}" "${stage_names[$index]}"
        total=$((total + stage_seconds[index]))
    done
    printf '    %5ds  total (all stages)\n' "$total"

    echo
    if [[ ${#skipped[@]} -eq 0 ]]; then
        echo "==> nothing was skipped"
    else
        echo "==> skipped, and why"
        for line in "${skipped[@]}"; do echo "    - $line"; done
    fi
}

# Ends the run when a stage failed: every failure, where its log is, and no green.
rd_stages_exit_if_failed() {
    [[ ${#failed_stages[@]} -gt 0 ]] || return 0
    echo >&2
    echo "!! ${#failed_stages[@]} stage(s) failed — the list is $CHECK_LOGS/failures:" >&2
    sed 's/^/   /' "$CHECK_LOGS/failures" >&2
    echo "   No green was recorded." >&2
    exit 1
}
