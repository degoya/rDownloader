# shellcheck shell=bash
# shellcheck disable=SC2034  # sets the flags check.sh reads
#
# What of a scripts/check.sh run a recorded green already covers (RD-160-06; owner, 2026-09-28:
# "unnötige Doppelprüfung immer vermeiden"), kept here so check.sh stays readable (RD-1120-06).
#
# A --full run records its halves `rust` and `web` by tree, --windows the half `windows`, --gate
# `clippy` and `windows`, --clippy-all `clippy`, --preflight `preflight` (scripts/lib/verified.sh).
# A half is covered when a green of it was recorded, by any checkout on the target, for this tree
# or for one that differs only in what that half does not read (rd_full_covering): documentation
# for every half, the generated web files for the lints (audit C1), scripts/ for the Rust tests.
#
#   * Every requested half covered — and for --full the preflight too — the run records the halves
#     for this tree and ends before the lock: rd_check_reuse_plan returns 0.
#   * Otherwise a --full runs only the halves no green covers (rd_check_reuse_apply), and skips
#     the script lints and tests when a preflight green covers the tree (audit C2): integrate.sh's
#     preflight ran them minutes before, and only the generators' output changed since.
#
# --again, or any argument but --full, --rust and --web, --windows and --gate alone, reuses
# nothing.

# Decides for checkout $1 and check.sh's arguments $2...; sets reuse (1 when the run may reuse),
# reuse_halves, covered_halves, covered_notes (half → note), preflight_covered and reuse_tree.
# Returns 0 when the whole run is covered, after recording it; 1 otherwise.
rd_check_reuse_plan() {
    local root="$1" argument half recorded
    shift
    reuse=0
    reuse_halves=(rust web)
    covered_halves=()
    declare -gA covered_notes=()
    preflight_covered=0
    reuse_tree=""
    for argument in "$@"; do
        case "$argument" in
            --full) [[ "$reuse" -eq -1 ]] || reuse=1 ;;
            --rust|--web)
                if [[ ${#reuse_halves[@]} -eq 2 ]]; then reuse_halves=("${argument#--}"); else reuse=-1; fi ;;
            *) reuse=-1 ;;
        esac
    done
    [[ "$*" != --windows ]] || { reuse=1; reuse_halves=(windows); }
    # The gate's two lints (RD-1100-13) are kept the same way, as the halves `clippy` and `windows`.
    [[ "$*" != --gate ]] || { reuse=1; reuse_halves=(clippy windows); }
    [[ "$reuse" -eq 1 ]] || return 1
    reuse_tree="$(rd_worktree_tree "$root")"
    [[ -n "$reuse_tree" ]] || return 1
    for half in "${reuse_halves[@]}"; do
        recorded="$(rd_full_covering "$root" "$half" "$reuse_tree")"
        [[ -n "$recorded" ]] || continue
        covered_halves+=("$half")
        covered_notes[$half]="tree ${recorded:0:12}, $(rd_covering_note "$root" "$half" "$recorded" "$reuse_tree")"
    done
    [[ -z "$(rd_full_covering "$root" preflight "$reuse_tree")" ]] || preflight_covered=1
    [[ ${#covered_halves[@]} -eq ${#reuse_halves[@]} ]] || return 1
    # --windows and --gate run no preflight stage; --full and its halves do.
    [[ "${reuse_halves[*]}" == windows || "${reuse_halves[*]}" == "clippy windows" || "$preflight_covered" -eq 1 ]] \
        || return 1
    rd_full_already_green "$root" "${reuse_halves[@]}" || return 1
    if [[ "${reuse_halves[*]}" == "rust web" ]]; then
        rd_record_verified "$root" "$(git -C "$root" rev-parse HEAD)"
        echo "==> recorded green at $(git -C "$root" rev-parse --short HEAD) in $(rd_verified_marker "$root")"
    fi
}

# For a --full run that rd_check_reuse_plan did not end: turns the covered halves off (run_rust,
# run_web), puts the green each relies on into rust_skip_reason and web_skip_reason for the skip
# list, and sets covered_rust and covered_web, so the green record still counts them.
rd_check_reuse_apply() {
    covered_rust=0
    covered_web=0
    [[ "$reuse" -eq 1 && "${reuse_halves[*]}" != windows && "${reuse_halves[*]}" != "clippy windows" ]] || return 0
    local half
    for half in "${covered_halves[@]+"${covered_halves[@]}"}"; do
        case "$half" in
            rust) run_rust=0; covered_rust=1; rust_skip_reason="a recorded green covers it (${covered_notes[rust]})" ;;
            web) run_web=0; covered_web=1; web_skip_reason="a recorded green covers it (${covered_notes[web]})" ;;
        esac
    done
}
