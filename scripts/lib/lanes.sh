# shellcheck shell=bash
#
# Where a checkout builds, and which lock guards that place (RD-140-06, parallel check lanes).
#
# Until 1.4 every worktree built into the main checkout's target/ and one lock serialised all of
# them, so a second check waited for the first however idle the machine was. A lane is a target
# directory of its own: a worktree created with `scripts/worktree.sh new --own-target` builds in
# `<worktree>/target`, the release chain cross-builds Windows in `target/lanes/windows`, and
# `scripts/lib/lock.sh` lets up to RD_LANES of them run at once — each lane still one run at a
# time, because two runs in one target directory are what the checkout stamp exists for.
#
# Sourced by lock.sh and verified.sh; defines functions only.
#
# Opt-in rather than the default, measured 2026-09-26 without compiling: one lane's debug
# working set is ~52 GiB (48 GiB of the newest variant per stem in target/debug/deps, 279 test
# binaries at up to 585 MB each, plus build/ and examples/), 596 GiB were free, and a wave had ten
# worktrees open. Ten own targets would need ~520 GiB; the two lanes the machine has memory for
# need ~105 GiB. The numbers are in docs/roadmap/jobs/archive/140-06-schneller-bauen-rest.md.

# The main checkout of the repository checkout $1 belongs to — itself for the main checkout.
rd_main_root() {
    local common
    common="$(git -C "$1" rev-parse --path-format=absolute --git-common-dir 2> /dev/null || true)"
    if [[ -n "$common" && "$common" != "$1/.git" ]]; then
        dirname "$common"
    else
        printf '%s\n' "$1"
    fi
}

# The flag that gives linked worktree $1 its own target directory. It lives in the worktree's
# own git directory (.git/worktrees/<name>/), so it is never tracked, never staged and goes away
# with `git worktree remove`.
rd_own_target_flag() {
    git -C "$1" rev-parse --path-format=absolute --git-path rd-own-target 2> /dev/null || true
}

# Whether checkout $1 is a linked worktree that builds in a target directory of its own.
rd_has_own_target() {
    local flag
    [[ "$(rd_main_root "$1")" != "$1" ]] || return 1
    flag="$(rd_own_target_flag "$1")"
    [[ -n "$flag" && -e "$flag" ]]
}

# The target directory of the checkout rooted at $1: where its build output and its green
# records live. CARGO_TARGET_DIR wins when set; a worktree with its own target uses
# `<worktree>/target`; every other linked worktree shares the main checkout's target/, which is
# the documented setup and no longer needs an exported variable to hold.
rd_target_dir() {
    if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
        printf '%s\n' "$CARGO_TARGET_DIR"
    elif rd_has_own_target "$1"; then
        printf '%s\n' "$1/target"
    else
        printf '%s\n' "$(rd_main_root "$1")/target"
    fi
}

# Where cargo writes for this run of checkout $1: RD_LANE_TARGET_DIR when a step builds in a lane
# of its own (the release chain's Windows package), otherwise the target directory above. The
# green records stay with rd_target_dir either way — a lane holds build output, not verdicts.
rd_build_dir() {
    printf '%s\n' "${RD_LANE_TARGET_DIR:-$(rd_target_dir "$1")}"
}

# Whether target directory $1 has never built a host or cross crate, so no foreign rlib can be
# in it and stamping the sources would only cost mtimes. The plugin components under
# wasm32-unknown-unknown do not count: an own target links them from the main checkout.
rd_target_is_fresh() {
    local profile
    for profile in "$1"/debug "$1"/release "$1"/x86_64-*/*; do
        [[ -d "$profile/.fingerprint" ]] && return 1
    done
    return 0
}

# The number of lanes, from RD_LANES (default 2). Two, because each lane builds with JOBS=4 and
# eight parallel rustc jobs is what this machine's 43 GB carry without swapping (RD-130-17).
rd_lanes() {
    local lanes="${RD_LANES:-2}"
    if [[ ! "$lanes" =~ ^[1-9][0-9]*$ ]]; then
        echo "RD_LANES must be a positive whole number, not '$lanes'" >&2
        return 2
    fi
    printf '%s\n' "$lanes"
}

# The lock that keeps two runs out of build directory $1. The main checkout's shared target keeps
# the lock file everybody knows — RD_LOCK_FILE, /tmp/rd-build.lock — so a checkout still on the
# single-lock scripts of 1.3 serialises against the new ones instead of building beside them.
# Every other build directory gets a file of its own beside it, named by a checksum of its path.
rd_target_lock_file() {
    local build_dir="$1" root="$2" base="${RD_LOCK_FILE:-/tmp/rd-build.lock}"
    if [[ "$build_dir" == "$(rd_main_root "$root")/target" ]]; then
        printf '%s\n' "$base"
    else
        printf '%s.target-%s\n' "$base" "$(printf '%s' "$build_dir" | cksum | cut -d' ' -f1)"
    fi
}

# The lock file of lane $1 (1 … RD_LANES).
rd_lane_lock_file() {
    printf '%s.lane%s\n' "${RD_LOCK_FILE:-/tmp/rd-build.lock}" "$1"
}

# Every target directory of the repository checkout $1 belongs to, one per line: the main
# checkout's target/, the lanes under it (target/lanes/<name>), and the target/ of every linked
# worktree that has its own. Only directories that exist. What scripts/prune-target.sh --all walks.
rd_all_target_dirs() {
    local main path lane
    main="$(rd_main_root "$1")"
    [[ -d "$main/target" ]] && printf '%s\n' "$main/target"
    for lane in "$main"/target/lanes/*/; do
        [[ -d "$lane" ]] && printf '%s\n' "${lane%/}"
    done
    while read -r path; do
        [[ -n "$path" && "$path" != "$main" && -d "$path/target" ]] || continue
        rd_has_own_target "$path" && printf '%s\n' "$path/target"
    done < <(git -C "$main" worktree list --porcelain 2> /dev/null | sed -n 's/^worktree //p')
}
