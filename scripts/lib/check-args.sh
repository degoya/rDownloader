# shellcheck shell=bash
# shellcheck disable=SC2034  # sets the flags check.sh reads
#
# The arguments of scripts/check.sh (RD-1280-08), kept here so check.sh stays under the length
# limit: the flags as globals — run_rust, run_web, full, defer, clippy_crates, clippy_all, windows,
# gate, again, preflight — and RUN_KIND, the run's kind for its stage logs (lib/stages.sh). Its
# usage is check.sh's header. Returns 2 on a usage error, after saying why.

rd_check_args() {
    run_rust=1
    run_web=1
    full=0
    defer=0
    clippy_crates=()
    clippy_all=0
    windows=0
    gate=0
    again=0
    preflight=0

    if [[ " $* " == *" --preflight "* && "$*" != --preflight && "$*" != "--preflight --again" \
        && "$*" != "--again --preflight" ]]; then
        echo "--preflight runs alone (--again aside)" >&2
        return 2
    fi
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --rust) run_web=0; shift ;;
            --web) run_rust=0; shift ;;
            --full) full=1; shift ;;
            --again) again=1; shift ;;
            --defer) defer=1; shift ;;
            --clippy-all) clippy_all=1; shift ;;
            --windows) windows=1; shift ;;
            --gate) gate=1; shift ;;
            --preflight) preflight=1; shift ;;
            --clippy)
                shift
                while [[ $# -gt 0 && "$1" != --* ]]; do clippy_crates+=("$1"); shift; done
                ;;
            *) echo "unknown argument: $1" >&2; return 2 ;;
        esac
    done

    if [[ "$full" -eq 1 && "$defer" -eq 1 ]]; then
        echo "--full and --defer are opposites" >&2
        return 2
    fi
    if [[ "$windows" -eq 1 || "$gate" -eq 1 ]] \
        && [[ "$full" -eq 1 || "$defer" -eq 1 || "$clippy_all" -eq 1 || ${#clippy_crates[@]} -gt 0 \
            || "$run_rust" -eq 0 || "$run_web" -eq 0 || "$windows$gate" == 11 ]]; then
        echo "--windows and --gate each run alone; start the other run separately" >&2
        return 2
    fi

    # Every stage is timed, skips are listed, failures collected (scripts/lib/stages.sh).
    RUN_KIND=branch
    if [[ "$full" -eq 1 ]]; then RUN_KIND=full; fi
    if [[ "$windows" -eq 1 ]]; then RUN_KIND=windows; fi
    if [[ "$gate" -eq 1 ]]; then RUN_KIND=gate; fi
    if [[ "$defer" -eq 1 ]]; then RUN_KIND=defer; fi
    if [[ "$preflight" -eq 1 ]]; then RUN_KIND=preflight; fi
}
