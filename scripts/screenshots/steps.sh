# shellcheck shell=bash
#
# The step loop of shots.sh (RD-1270-24): the steps in the order given, each its `step_<name>`.
# A failed step ends the run, because the steps after it would work on a state that is not
# there — `fresh` refused a leftover work directory, and `start seed capture stop` went on to
# seed the old service a second time and photograph every package twice. A `stop` named later
# still runs when this run ran `start`, so no service it started is left behind. Sourced by
# shots.sh, and by scripts/tests/screenshots.sh with stand-in steps.
#
# run_steps <step>...: 0 when every step passed, 1 after the first that failed.
run_steps() {
    local step failed="" started=0
    for step in "$@"; do
        if [[ -n "$failed" ]]; then
            if [[ "$step" == stop && "$started" -eq 1 ]]; then
                echo "stop: after $failed failed, so the service this run started does not stay" >&2
                step_stop || true
            else
                echo "skipped: $step, after $failed failed" >&2
            fi
            continue
        fi
        [[ "$step" == start ]] && started=1
        "step_$step" || failed="$step"
    done
    if [[ -n "$failed" ]]; then
        echo "failed: $failed" >&2
        return 1
    fi
}
