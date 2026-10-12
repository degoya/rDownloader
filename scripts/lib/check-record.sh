#!/usr/bin/env bash
# shellcheck shell=bash
# shellcheck disable=SC2034,SC2154  # sets stages_recorded_note; reads check.sh's flags
#
# The --full green, recorded per half and by tree (scripts/lib/verified.sh), kept here so check.sh
# stays readable. A half is recorded when its own stages all ended with 0 — also when another half
# is red (RD-1270-22): round 2 of integration/1.27-w1 was red only in Vitest under load, and the
# next --full built and tested the green Rust half again, about 17 minutes, because a red run
# ended before anything was recorded. Now that run keeps the green half, and the next --full over
# the same tree reuses it (lib/check-reuse.sh) and runs only the red one. The revision
# (rd_record_verified) stays bound to both halves green; check.sh records it only after
# rd_stages_exit_if_failed.
#
# Which half a stage belongs to is said once, where it runs: check.sh sets `stage_half` for its
# Rust section, rd_check_web for the web half (lib/check-web.sh), and lib/stages.sh records the
# half of every failed stage in failed_halves — `preflight` for every stage without one: git diff
# --check, the job layout, the version copies, the script lints and tests, gitleaks. cargo fmt is
# the preflight's as well as the Rust half's.
#
# Expects from the caller: `full`, `full_tree`, `run_rust`, `covered_rust`, `run_web`,
# `covered_web`, `crate_reuse_base` (when lib/check-reuse-crates.sh set it) and `ROOT`, as check.sh
# defines them, and lib/stages.sh and lib/verified.sh sourced.

# Records every half of a --full run whose stages were green for the tree taken before the run,
# and says which; on a red run sets stages_recorded_note for rd_stages_exit_if_failed.
rd_check_record_halves() {
    [[ "$full" -eq 1 && -n "$full_tree" ]] || return 0
    local half recorded=() red=()
    local -A ran=([rust]=$((run_rust | covered_rust)) [web]=$((run_web | covered_web)) [preflight]=1)
    for half in rust web preflight; do
        if [[ " ${failed_halves[*]-} " == *" $half "* ]]; then red+=("$half"); continue; fi
        [[ "${ran[$half]}" -eq 1 ]] || continue
        rd_record_full "$ROOT" "$half" "$full_tree"
        recorded+=("$half")
    done
    if [[ ${#red[@]} -eq 0 ]]; then
        echo "==> recorded a --full green for tree ${full_tree:0:12} in $(rd_full_marker "$ROOT")$([[ -z "${crate_reuse_base:-}" ]] \
            || echo ", its Rust half per crate on the green of tree ${crate_reuse_base:0:12}")"
        return 0
    fi
    [[ ${#recorded[@]} -gt 0 ]] || return 0
    echo
    echo "==> recorded $(rd_check_record_halves_named "${recorded[@]}") for tree ${full_tree:0:12} in $(rd_full_marker "$ROOT") although $(rd_check_record_halves_named "${red[@]}") failed;"
    echo "    a --full over this tree reuses what was recorded and runs only the rest (lib/check-reuse.sh)"
    stages_recorded_note="No revision was recorded green; only $(rd_check_record_halves_named "${recorded[@]}") of tree ${full_tree:0:12}, named above."
}

# "the rust half", "the rust and preflight halves", "the rust, web and preflight halves".
rd_check_record_halves_named() {
    if [[ $# -eq 1 ]]; then echo "the $1 half"; return 0; fi
    local joined
    joined="$(printf '%s, ' "${@:1:$#-1}")"
    echo "the ${joined%, } and ${*: -1} halves"
}
