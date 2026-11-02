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
        # shellcheck source=tool-versions.sh
        source "$ROOT/scripts/lib/tool-versions.sh"
        echo "cargo xwin is not installed (cargo install cargo-xwin --version $RD_CARGO_XWIN_VERSION --locked)" >&2
        return 1
    fi
    dir="$(rd_lint_windows_dir)"
    for manifest in crates/*/Cargo.toml; do args+=(-p "$(basename "$(dirname "$manifest")")"); done
    echo "==> cargo xwin clippy for x86_64-pc-windows-msvc over $(( ${#args[@]} / 2 )) crates (--keep-going, -j 2) in $dir"
    mkdir -p "$dir"
    # The one `flock` outside lib/lock.sh, and on purpose (RD-1280-15): rd_take_lock and
    # rd_take_lane run the whole script again under the lock, for a script's start; this is one
    # command inside a run that already holds the checkout target's lock, and needs the lane's
    # lock around exactly it, which prune-target.sh takes too. Never wrapped around a script.
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

# --- check.sh --full's lint: from the last clippy green to this tree (RD-1410-04) ---------------
#
# --full runs no whole-workspace lint; the integration gate records `clippy` for the gate's tree.
# A release starts from a later tree: release-start.sh wants the migration baselines committed
# (scripts/migration-baselines.sh, two lines under crates/rd-db/tests/database/), and after a
# lockfile change integrate.sh regenerates the licence list once the gate is through. The --full
# on that tree is the one run it gets, and it linted nothing, so no `clippy` green ever covered the
# tree before the bump, and the release chain's clippy step (rd_prebump_reuse) ran the whole
# workspace in every release from 1.38.0 to 1.40.1.
#
# So --full lints what changed since the closest recorded clippy green: placed as the reuse per
# crate places it (lib/crate-graph.sh, rd_crate_demands), a member's own targets for its test code,
# the whole reverse hull of a member whose library changed, and records the `clippy` half for the
# tree when that passed. A wide path (a manifest, the lock file, rd-core, a build script, ...), a
# hull that takes in rd-api (whose integration binaries are never linted all at once), or rd-api
# test code outside tests/<binary>/ leaves it as before: nothing linted, a skip. The limits are the
# reuse per crate's: features are unified over the selected members, not the workspace.

# Files a Rust source reads as data only (include_str! of text parsed at run time): their content
# cannot change what clippy says, so the bridge passes them over. The licence list, which
# integrate.sh writes after the gate.
RD_LINT_NEUTRAL_FILES=(crates/rd-api/licenses/third-party.json)

# What --full lints for tree $2 of checkout $1, the working directory at that checkout: the
# recorded clippy green it builds on on the first line, then one `cargo clippy` selection per line
# (`-p <package>... --all-targets`, `-p rd-api --test <binary>...`). Only the first line when a
# green covers the tree (rd_full_covering) or what changed is data RD_LINT_NEUTRAL_FILES names;
# nothing when no green of the newest RD_FULL_CANDIDATES_MAX is within reach.
rd_lint_bridge_plan() {
    local checkout="$1" tree="$2" candidate compared=0 covered
    [[ -n "$tree" ]] || return 0
    covered="$(rd_full_covering "$checkout" clippy "$tree")"
    if [[ -n "$covered" ]]; then
        printf '%s\n' "$covered"
        return 0
    fi
    rd_crate_load_graph
    while IFS= read -r candidate; do
        [[ -n "$candidate" ]] || continue
        [[ "$compared" -lt "$RD_FULL_CANDIDATES_MAX" ]] || return 0
        compared=$((compared + 1))
        rd_lint_bridge_from "$checkout" "$candidate" "$tree" && return 0
    done < <(rd_full_recorded "$checkout" clippy)
    return 0
}

# rd_lint_bridge_plan's answer for the clippy green of tree $2 against tree $3 of checkout $1;
# fails, printing nothing, when that green is out of reach. Needs rd_crate_load_graph.
rd_lint_bridge_from() {
    local checkout="$1" green="$2" tree="$3" paths demands kind package path
    local -a demanded=() hull=() own=() binaries=() selected=()
    paths="$(git -C "$checkout" diff --no-renames --name-only "$green" "$tree" 2> /dev/null)" || return 1
    paths="$(rd_paths_read_by clippy <<< "$paths" \
        | grep -vxF -f <(printf '%s\n' "${RD_LINT_NEUTRAL_FILES[@]}") || true)"
    if [[ -n "$paths" ]]; then
        rd_crate_first_wide <<< "$paths"
        [[ -z "$RD_CRATE_FIRST_WIDE" ]] || return 1
        demands="$(RD_CRATE_FROM="$green" RD_CRATE_TO="$tree" rd_crate_demands <<< "$paths")"
        ! grep -q '^wide ' <<< "$demands" || return 1
        mapfile -t demanded < <(sed -n 's/^hull \([^ ]*\) .*/\1/p' <<< "$demands" | LC_ALL=C sort -u)
        mapfile -t hull < <(rd_crate_reverse_hull "${demanded[@]+"${demanded[@]}"}")
        ! printf '%s\n' "${hull[@]+"${hull[@]}"}" | grep -qx rd-api || return 1
        while read -r kind package path _; do
            [[ "$kind" == own ]] || continue
            if [[ "$package" != rd-api ]]; then
                own+=("$package")
            elif [[ "$path" =~ ^crates/rd-api/tests/([^/]+)/ && -f "crates/rd-api/tests/${BASH_REMATCH[1]}/main.rs" ]]; then
                binaries+=("${BASH_REMATCH[1]}")
            else
                return 1
            fi
        done <<< "$demands"
    fi
    printf '%s\n' "$green"
    mapfile -t selected < <(printf '%s\n' "${hull[@]+"${hull[@]}"}" "${own[@]+"${own[@]}"}" | sed '/^$/d' | LC_ALL=C sort -u)
    if [[ ${#selected[@]} -gt 0 ]]; then
        printf -- '-p %s ' "${selected[@]}"
        echo "--all-targets"
    fi
    if [[ ${#binaries[@]} -gt 0 ]]; then
        mapfile -t binaries < <(printf '%s\n' "${binaries[@]}" | LC_ALL=C sort -u)
        printf -- '-p rd-api'
        printf -- ' --test %s' "${binaries[@]}"
        echo
    fi
}

# check.sh --full's lint for tree $1 (above): each selection of rd_lint_bridge_plan a stage, the
# `clippy` half recorded for the tree when all passed; a skip when no green is within reach. Needs
# check.sh's ROOT and JOBS, lib/stages.sh, lib/verified.sh, lib/scope.sh and lib/crate-graph.sh,
# and the working directory at the checkout root.
rd_lint_full() {
    local tree="$1" plan green line failed_before=${#failed_stages[@]}
    local -a lines=() args=()
    plan="$(rd_lint_bridge_plan "$ROOT" "$tree")"
    if [[ -z "$plan" ]]; then
        skip "clippy" "no recorded clippy green is within reach of a lint per crate; scripts/check.sh --gate (integrate.sh) and CI's rust job lint the workspace, --clippy-all here"
        return 0
    fi
    mapfile -t lines <<< "$plan"
    green="${lines[0]}"
    for line in "${lines[@]:1}"; do
        read -ra args <<< "$line"
        step "clippy on ${args[*]}: what changed since the clippy green of tree ${green:0:12}"
        attempt env CARGO_BUILD_JOBS="$JOBS" cargo clippy "${args[@]}" --all-features -j "$JOBS" -- -D warnings
    done
    [[ ${#failed_stages[@]} -eq "$failed_before" ]] || return 0
    [[ ${#lines[@]} -gt 1 ]] || echo "==> clippy: the green of tree ${green:0:12} covers this tree; nothing to lint"
    rd_record_full "$ROOT" clippy "$tree"
    echo "    recorded the clippy green for tree ${tree:0:12} in $(rd_full_marker "$ROOT")"
}
