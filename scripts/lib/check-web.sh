#!/usr/bin/env bash
# shellcheck shell=bash
# shellcheck disable=SC2154  # the change flags are check.sh's, which sources this file
#
# The web and extension half of scripts/check.sh, kept here so check.sh stays readable
# (RD-1100-12 T16): the full typecheck, vitest and the build of web/ when it changed, the
# extension's build when extension/ did, each command through `attempt`.
#
# Expects from the caller: `run_web`, `web_changed`, `extension_changed`, `step`, `skip` and
# `attempt`, as check.sh defines them (`web_skip_reason` when lib/check-reuse.sh set one,
# `web_locales_only` when lib/check-reuse-crates.sh did), and the working directory at the checkout
# root.

# Vitest, with the worker bound rd_check_web_start sets beside the Rust half (RD-1270-23); alone,
# its own default. Passed straight through: the `test` script is `vitest run`, and both projects of
# web/vitest.config.ts share the one pool the flag sizes.
rd_check_vitest() {
    step "pnpm run test${vitest_workers:+ --maxWorkers=$vitest_workers}"
    attempt pnpm --dir web run test ${vitest_workers:+"--maxWorkers=$vitest_workers"}
}

rd_check_web() {
    # Every stage here is the web half's (lib/stages.sh, lib/check-record.sh).
    # shellcheck disable=SC2034  # read by lib/stages.sh's attempt
    local stage_half=web
    # Only translation catalogues since a web green (RD-1150-06): Vitest, which loads every one.
    if [[ "$run_web" -eq 1 && "${web_locales_only:-0}" -eq 1 ]]; then
        local base="the web green of tree ${web_locales_base:0:12}"
        if [[ "${web_locales_typecheck:-0}" -eq 1 ]]; then
            step "pnpm run typecheck:full"
            attempt pnpm --dir web run typecheck:full
        else
            skip "pnpm run typecheck:full" "only translation catalogues changed since $base, and no source imports one by name"
        fi
        rd_check_vitest
        skip "pnpm run build" "only translation catalogues changed since $base; Vitest loaded every one, GitHub CI and the release build the frontend"
        skip "the browser extension" "$base covers it; nothing under extension/ changed since"
        return 0
    fi
    if [[ "$run_web" -eq 1 && "$web_changed" -eq 1 ]]; then
        # Non-incremental on every run, the one CI and the release chain run: the incremental
        # `typecheck` trusts web/tsconfig.*.tsbuildinfo, and two type errors it passed reached
        # GitHub on 2026-09-27 (RD-150-22).
        step "pnpm run typecheck:full"
        attempt pnpm --dir web run typecheck:full
        rd_check_vitest
        # Refused rather than run-and-warn. A feature worktree links web/dist to the main
        # checkout's (scripts/worktree.sh), and a build would empty and rewrite that one. Its own
        # web/node_modules (RD-150-14) is no longer a hazard: until 1.5 that was a link too, and
        # the unplugin generators wrote the other checkout's paths into the tracked declarations.
        #
        # Not built here, deliberately, also not by --full in an integration worktree (audit C7):
        # rd-api's `/` tests embed web/dist through rust-embed, which keeps serving the main
        # checkout's once compiled through the link (scripts/lib/web-dist.sh), so a build of this
        # branch's frontend would change nothing they see. The build of this content is GitHub's
        # (scripts/public-ci.sh, which integrate.sh --public-ci starts beside --full).
        if [[ -L web/dist ]]; then
            skip "pnpm run build" "a feature worktree: web/dist links the main checkout's, and rd-api's / tests ran against that frontend, not this branch's; GitHub CI builds this one (scripts/public-ci.sh)"
            echo
            echo "    pnpm run build is refused here: it would write into the main checkout's web/dist."
            echo "    'rm web/dist' (the link only) first to build in this worktree."
        elif scripts/web-dist-stale.sh > /dev/null; then
            skip "pnpm run build" "web/dist is newer than every source that goes into it"
        else
            step "pnpm run build"
            attempt pnpm --dir web run build
            # Belt and braces: these are generated and tracked, so a surprise diff is worth naming
            # even outside a worktree — it usually means the component inventory really did change
            # and the regenerated files belong in the commit.
            if ! git diff --quiet -- web/components.d.ts web/auto-imports.d.ts; then
                echo
                echo "!! web/components.d.ts or web/auto-imports.d.ts changed — review and commit them." >&2
            fi
        fi
    elif [[ "$run_web" -eq 1 ]]; then
        skip "typecheck, vitest and the web build" "nothing under web/ changed"
    fi

    if [[ "$run_web" -eq 1 && "$extension_changed" -eq 1 ]]; then
        step "browser extension"
        attempt scripts/build-extension.sh
    elif [[ "$run_web" -eq 1 ]]; then
        skip "the browser extension" "nothing under extension/ changed"
    fi
    if [[ "$run_web" -eq 0 ]]; then skip "the whole web half" "${web_skip_reason:---rust was given}"; fi
}

# --- beside the Rust half (RD-1270-13) ---------------------------------------------------------
#
# The web half reads web/ and extension/ and writes nothing the Rust half reads — except
# `pnpm run build`, whose web/dist rust-embed compiles into rd-api. So when both halves run and the
# web half would not build web/dist (a worktree's link, a web/dist newer than its sources, nothing
# under web/ changed), it runs beside the Rust half instead of after it: its output into
# $CHECK_LOGS/<kind>-web.out, printed whole once the Rust half ended, its stage logs as
# <kind>-web-NN.log, its stage times, skips and failures joined to the run's. A run that builds
# web/dist keeps the order, and so does RD_WEB_SERIAL=1, should memory run short. Beside it,
# Vitest gets RD_VITEST_WORKERS workers (scripts/lib/jobs.sh, RD-1270-23): its default of the cores
# less one, next to nextest's TEST_THREADS, ran tests past their limit without one being wrong.

# Whether rd_check_web would run `pnpm run build` (the conditions it applies, in its order).
rd_check_web_builds() {
    [[ "$run_web" -eq 1 && "${web_locales_only:-0}" -ne 1 && "$web_changed" -eq 1 && ! -L web/dist ]] \
        && ! scripts/web-dist-stale.sh > /dev/null
}

# Starts the web half in the background when it may run beside the Rust half; sets web_pid.
rd_check_web_start() {
    web_pid=""
    [[ "$run_rust" -eq 1 && "$run_web" -eq 1 && "${RD_WEB_SERIAL:-0}" != 1 ]] || return 0
    ! rd_check_web_builds || return 0
    close_stage
    web_out="$CHECK_LOGS/$RUN_KIND-web.out"
    web_state="$CHECK_LOGS/$RUN_KIND-web.state"
    rm -f "$web_out" "$web_state" "$CHECK_LOGS/failures.web"
    (
        stage_names=() stage_seconds=() skipped=() failed_stages=() failed_halves=()
        # shellcheck disable=SC2034  # lib/stages.sh's
        RUN_KIND="$RUN_KIND-web" stage_failures="$CHECK_LOGS/failures.web"
        vitest_workers="${RD_VITEST_WORKERS:-}"
        rd_check_web
        close_stage
        declare -p stage_names stage_seconds skipped failed_stages failed_halves > "$web_state"
    ) > "$web_out" 2>&1 < /dev/null &
    web_pid=$!
    # A run that ends before the join (a map check's `exit 1`, errexit) takes the web half along.
    trap rd_check_web_abandon EXIT
    echo
    echo "==> the web half runs beside the Rust half, Vitest with ${RD_VITEST_WORKERS:-its default} workers (RD_VITEST_WORKERS);"
    echo "    its output follows the Rust half ($web_out)"
}

rd_check_web_abandon() {
    [[ -n "${web_pid:-}" ]] || return 0
    rd_kill_tree "$web_pid"
    echo "!! the web half was stopped: the run ended before it did (its output so far: $web_out)" >&2
}

# Process $1 and everything under it, the children first (by parent PID, never by command line).
rd_kill_tree() {
    local child
    for child in $(pgrep -P "$1" 2> /dev/null || true); do rd_kill_tree "$child"; done
    kill "$1" 2> /dev/null || true
}

# The web half: after the Rust half as before, or the end of the one started beside it, whose
# output is printed here and whose stages join the run's.
rd_check_web_join() {
    if [[ -z "${web_pid:-}" ]]; then
        rd_check_web
        return 0
    fi
    close_stage
    wait "$web_pid" || true
    web_pid=""
    trap - EXIT
    echo
    echo "==> the web half, which ran beside the Rust half:"
    cat "$web_out"
    if [[ -r "$web_state" ]]; then
        # shellcheck disable=SC1090
        source <(sed -E 's/^declare -a ([a-z_]+)=/declare -ga web_\1=/' "$web_state")
        stage_names+=(${web_stage_names[@]+"${web_stage_names[@]}"})
        stage_seconds+=(${web_stage_seconds[@]+"${web_stage_seconds[@]}"})
        skipped+=(${web_skipped[@]+"${web_skipped[@]}"})
        failed_stages+=(${web_failed_stages[@]+"${web_failed_stages[@]}"})
        failed_halves+=(${web_failed_halves[@]+"${web_failed_halves[@]}"})
    else
        failed_stages+=("the web half")
        failed_halves+=(web)
        echo "the web half — ended without its state, log $web_out" >> "$CHECK_LOGS/failures"
    fi
    if [[ -s "$CHECK_LOGS/failures.web" ]]; then cat "$CHECK_LOGS/failures.web" >> "$CHECK_LOGS/failures"; fi
    rm -f "$CHECK_LOGS/failures.web" "$web_state"
}
