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
# non-zero after its last stage, with the list, and records no green — a --full only the halves
# whose own stages were green (lib/check-record.sh, RD-1270-22). Until 1.10 the first red
# test ended a --full run, and every further failure cost another round of 10 to 16 minutes.
#
# Not every stage goes through it: only the gates that make the rest meaningless still stop the
# run at once — the rd-api test map and the Rust test inputs map, and components that do not
# build. The script checks collect like every other stage since RD-1110-15: each lint command and
# each script test is a stage of its own.
#
# Expects from the caller: CHECK_LOGS (a directory) and RUN_KIND (branch, full, windows, gate),
# set before rd_stages_init. A half that runs beside the rest (lib/check-web.sh) sets its own
# RUN_KIND and stage_failures, and its stages join the run's when it ended.
#
#   step "name"            # closes the previous stage, opens this one
#   skip "what" "why"      # listed at the end under "skipped, and why"
#   attempt cmd args...    # runs cmd in the current stage; a failure is recorded, not fatal
#   rd_stages_parallel "name=cmd"...  # stages side by side, each with its own log
#   rd_stages_report       # the time per stage and the skipped list
#   rd_stages_exit_if_failed
#
# Each failed stage also records its half in failed_halves (RD-1270-22): `stage_half` as the caller
# set it where the stages run — `rust` in check.sh's Rust section, `web` in rd_check_web — and
# `preflight` for every other stage, so a red --full still records the halves that were green.

stage_names=()
stage_seconds=()
stage_name=""
stage_started=0
skipped=()
failed_stages=()
failed_halves=()

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
    failed_halves+=("${stage_half:-preflight}")
    {
        echo "$stage_name — exit $status, log $log"
        rd_stage_failure_lines "$log" | sed 's/^/    /'
    } >> "${stage_failures:-$CHECK_LOGS/failures}"
    echo "!! $stage_name failed (exit $status); the run goes on and lists it at the end" >&2
}

# Runs stages side by side (RD-1270-13): each argument is `<stage name>=<command>`, a function or
# a program without arguments. Each writes to its own stage log only, never to the terminal, so
# the outputs do not mix; once all ended, each log is printed in turn and each stage counted like
# one `attempt` ran — its time, and on a failure its entry in the list. parallel_status holds the
# exit statuses in argument order.
# shellcheck disable=SC2034  # parallel_status is the caller's to read
parallel_status=()
# shellcheck disable=SC2034
rd_stages_parallel() {
    local entry index status seconds first=$(( ${#stage_names[@]} + 1 ))
    local -a names=() commands=() logs=() pids=()
    close_stage
    for entry in "$@"; do names+=("${entry%%=*}"); commands+=("${entry#*=}"); done
    for index in "${!names[@]}"; do
        logs[index]="$CHECK_LOGS/$RUN_KIND-$(printf '%02d' $((first + index))).log"
        echo
        echo "==> ${names[index]} (side by side; output in ${logs[index]} until all ended)"
        (
            started=$SECONDS
            status=0
            "${commands[index]}" > "${logs[index]}" 2>&1 < /dev/null || status=$?
            echo "$status $((SECONDS - started))" > "${logs[index]}.end"
        ) &
        pids[index]=$!
    done
    parallel_status=()
    for index in "${!names[@]}"; do
        wait "${pids[index]}" || true
        read -r status seconds < "${logs[index]}.end" 2> /dev/null || { status=1; seconds=0; }
        rm -f "${logs[index]}.end"
        parallel_status[index]="$status"
        stage_names+=("${names[index]}")
        stage_seconds+=("$seconds")
        echo
        echo "==> ${names[index]}: exit $status after ${seconds}s, its log:"
        cat "${logs[index]}"
        [[ "$status" -ne 0 ]] || continue
        failed_stages+=("${names[index]}")
        failed_halves+=("${stage_half:-preflight}")
        {
            echo "${names[index]} — exit $status, log ${logs[index]}"
            rd_stage_failure_lines "${logs[index]}" | sed 's/^/    /'
        } >> "${stage_failures:-$CHECK_LOGS/failures}"
        echo "!! ${names[index]} failed (exit $status); the run goes on and lists it at the end" >&2
    done
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

# Ends the run when a stage failed: every failure, where its log is, and no green — or only the
# halves lib/check-record.sh recorded, which it names in stages_recorded_note.
rd_stages_exit_if_failed() {
    [[ ${#failed_stages[@]} -gt 0 ]] || return 0
    echo >&2
    echo "!! ${#failed_stages[@]} stage(s) failed — the list is $CHECK_LOGS/failures:" >&2
    sed 's/^/   /' "$CHECK_LOGS/failures" >&2
    echo "   ${stages_recorded_note:-No green was recorded.}" >&2
    exit 1
}
