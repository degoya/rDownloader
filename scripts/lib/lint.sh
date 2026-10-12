#!/usr/bin/env bash
# shellcheck shell=bash
# shellcheck disable=SC2154  # ROOT, again, reuse_tree, failed_stages, parallel_status: check.sh's and its libraries
#                             # (reuse_tree: the tree lib/check-reuse.sh took inside the lock, RD-1270-13)
#
# The two whole-workspace lints of scripts/check.sh, kept here so check.sh stays readable: clippy
# for Linux (`--clippy-all`, and the Linux half of `--gate`) and `cargo xwin clippy` for Windows
# (`--windows`, and the Windows half of `--gate`, RD-140-23). Every crate, all targets and all
# features, which is what CI's `rust` job lints, at -j 2: the whole workspace at once has taken
# WSL into swap at higher parallelism.
#
# Both with `--keep-going` (RD-1100-13): a lint that stops at the first crate that does not
# compile shows that crate's errors and nothing else, and the 1.9.1 integration needed four fix
# rounds for 24 files that way. With it, cargo builds every crate it still can and one round
# shows every error.
#
# Expects the lock taken and the working directory at the checkout root. Each returns cargo's
# exit status; the caller decides what a red one means. Below them the runs of check.sh that
# consist of them, --windows and --gate, which need check.sh's ROOT and `again`, lib/stages.sh and
# lib/verified.sh.

rd_lint_linux() {
    echo "==> cargo clippy over the workspace, all targets and features (--keep-going, -j 2)"
    CARGO_BUILD_JOBS=2 cargo clippy --workspace --all-targets --all-features --keep-going -j 2 \
        -- -D warnings
}

# The Windows lint's own target directory (RD-1270-13), a lane of the checkout's target: cargo
# locks a build directory's host profile for every build in it, a cross build too, so the two lints
# in one target/ would run one after the other whatever the script did. The runs that use it hold
# the checkout target's lock (check.sh --windows and --gate); scripts/prune-target.sh prunes it
# under its own lock, which rd_lint_windows holds around cargo.
rd_lint_windows_dir() {
    printf '%s/lanes/windows-lint\n' "$(rd_build_dir "$ROOT")"
}

# Stamps crates/ when the Windows lint's directory last served another checkout, as lib/lock.sh
# does for the target itself. Before the lints start: touching the sources under a running build
# would leave the Linux lint stale.
rd_lint_windows_stamp() {
    RD_LANE_TARGET_DIR="$(rd_lint_windows_dir)" rd_stamp_sources
}

# Every crate by name: `--` arguments reach the selected packages only. All targets and features,
# because tests compile differently on Windows (RD-120-67).
rd_lint_windows() {
    local manifest dir
    local -a args=()
    if ! cargo xwin --version > /dev/null 2>&1; then
        echo "cargo xwin is not installed (cargo install cargo-xwin --version 0.23.1 --locked)" >&2
        return 1
    fi
    dir="$(rd_lint_windows_dir)"
    for manifest in crates/*/Cargo.toml; do args+=(-p "$(basename "$(dirname "$manifest")")"); done
    echo "==> cargo xwin clippy for x86_64-pc-windows-msvc over $(( ${#args[@]} / 2 )) crates (--keep-going, -j 2) in $dir"
    mkdir -p "$dir"
    CARGO_TARGET_DIR="$dir" CARGO_BUILD_JOBS=2 flock -w "${RD_LOCK_WAIT:-7200}" -E 199 \
        "$(rd_target_lock_file "$dir" "$ROOT")" \
        cargo xwin clippy --target x86_64-pc-windows-msvc -j 2 "${args[@]}" \
        --all-targets --all-features --keep-going -- -D warnings
}

# Runs lint $3... as a stage and, when it passed, records half $1 (`clippy` or `windows`) for tree
# $2 (scripts/lib/verified.sh) — the gate's halves and --clippy-all's (audit C8). Needs `attempt`
# and `failed_stages` (lib/stages.sh).
rd_lint_recorded() {
    local half="$1" tree="$2" failed_before=${#failed_stages[@]}
    shift 2
    attempt "$@"
    if [[ ${#failed_stages[@]} -eq "$failed_before" && -n "$tree" ]]; then
        rd_record_full "$ROOT" "$half" "$tree"
        echo "    recorded the $half green for tree ${tree:0:12} in $(rd_full_marker "$ROOT")"
    fi
}

# Records half $1 for tree $2 when its parallel stage ended with status $3 = 0.
rd_lint_record_parallel() {
    [[ "$3" -eq 0 && -n "$2" ]] || return 0
    rd_record_full "$ROOT" "$1" "$2"
    echo "    recorded the $1 green for tree ${2:0:12} in $(rd_full_marker "$ROOT")"
}

# check.sh --windows: the Windows half of the workspace (RD-140-23). Nothing else in check.sh
# reads `cfg(windows)` code, and v1.3.0 shipped with 42 Windows test failures that only GitHub's
# runner found. Clippy links nothing, so this is minutes (4m40s at -j 2 on 2026-09-25, 76-85 s
# warm), not the hour a Windows test run would be. Its green is recorded by tree as the half
# `windows` (RD-160-06), never as a revision: it verifies one platform's lint, and a later
# --windows over content it covers ends before the lock (lib/check-reuse.sh). Exits.
rd_check_windows() {
    local tree
    tree="${reuse_tree:-$(rd_worktree_tree "$ROOT")}"
    rd_lint_windows_stamp
    step "the Windows lint"
    rd_lint_recorded windows "$tree" rd_lint_windows
    rd_stages_report
    rd_stages_exit_if_failed
    echo "==> all requested checks passed"
    exit 0
}

# check.sh --gate, the integration gate (RD-1100-13): both whole-workspace lints, before
# scripts/integrate.sh runs a generator. The 1.9.1 integration's first run died in api-contract.sh
# on the first compile error, and four fix rounds over 24 files followed; here both lints run with
# --keep-going and the second whatever the first said, so one round shows every error of both
# platforms. Each green is recorded by tree, as the halves `clippy` and `windows`, and a half its
# green already covers is not run again (--again runs it anyway). Exits.
#
# Side by side (RD-1270-13; 87 s and 79 s one after the other before), each a stage with its own
# log (rd_stages_parallel), two cargo runs at -j 2 within the one lane; RD_GATE_SERIAL=1 runs them
# one after the other, should memory run short.
rd_check_gate() {
    local tree half
    local -a todo=()
    tree="${reuse_tree:-$(rd_worktree_tree "$ROOT")}"
    for half in clippy windows; do
        if [[ "$again" -eq 0 && -n "$tree" && -n "$(rd_full_covering "$ROOT" "$half" "$tree")" ]]; then
            echo "==> $half: a recorded green covers this content; not run again"
            continue
        fi
        todo+=("$half")
    done
    [[ " ${todo[*]-} " != *" windows "* ]] || rd_lint_windows_stamp
    if [[ ${#todo[@]} -eq 2 && "${RD_GATE_SERIAL:-0}" != 1 ]]; then
        rd_stages_parallel "the Linux lint=rd_lint_linux" "the Windows lint=rd_lint_windows"
        rd_lint_record_parallel clippy "$tree" "${parallel_status[0]}"
        rd_lint_record_parallel windows "$tree" "${parallel_status[1]}"
    else
        for half in "${todo[@]+"${todo[@]}"}"; do
            if [[ "$half" == clippy ]]; then
                step "the Linux lint"
                rd_lint_recorded clippy "$tree" rd_lint_linux
            else
                step "the Windows lint"
                rd_lint_recorded windows "$tree" rd_lint_windows
            fi
        done
    fi
    rd_stages_report
    rd_stages_exit_if_failed
    echo "==> all requested checks passed"
    exit 0
}
